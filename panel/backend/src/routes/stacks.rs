use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, Claims, ServerScope};
use crate::services::expected_stops;
use crate::error::{internal_error, err, agent_error, require_admin, ApiError};
use crate::services::activity;
use crate::services::agent::AgentHandle;
use crate::services::domain_claim::{self, Holder};
use crate::AppState;

use super::tls_certificates::{
    certificate_id_for_alias, is_valid_cert_alias, require_agent_at_least, PROVIDED_TLS_MIN_AGENT,
};

/// The row plus the alias of the certificate it references, if any. A LEFT JOIN
/// because most stacks reference none, and the alias is what the API and the
/// deploy body speak — the id never leaves the panel.
const STACK_SELECT: &str = "SELECT s.id, s.user_id, s.server_id, s.name, s.yaml, s.service_count, \
                            s.domain, s.ssl_email, s.tls_mode, s.tls_certificate_id, \
                            c.alias AS tls_certificate, s.created_at, s.updated_at \
                            FROM docker_stacks s \
                            LEFT JOIN tls_certificates c ON c.id = s.tls_certificate_id";

/// The one vocabulary for how a stack's domain is served. Pinned cross-tree:
/// the migration's CHECK, the agent's request parser and the SPA's select all
/// carry these three words.
const TLS_MODES: [&str; 3] = ["none", "acme", "provided"];

/// The mode a stack is in, from what the row says.
///
/// The stored value wins. A NULL is a row written by an older binary — before
/// the column existed, or after a rollback — and for such a row the address is
/// the mode, exactly as the agent has always inferred it: a non-blank
/// `ssl_email` means Let's Encrypt, anything else means plain HTTP.
pub(crate) fn effective_tls_mode(tls_mode: Option<&str>, ssl_email: Option<&str>) -> &'static str {
    if let Some(stored) = tls_mode {
        if let Some(known) = TLS_MODES.iter().find(|m| **m == stored) {
            return known;
        }
    }
    let has_email = ssl_email.map(str::trim).filter(|e| !e.is_empty()).is_some();
    if has_email {
        "acme"
    } else {
        "none"
    }
}

/// A requested mode, normalised, or a 400 for a word outside the vocabulary.
/// `None` in means "the client did not say" — the caller decides what that
/// means (derived from the address on create, the stored mode on update).
///
/// `pub(crate)` since s414: reused by `routes::docker_apps::deploy` for
/// template apps, the sibling of stacks that had no way to request `provided`
/// mode at all.
pub(crate) fn requested_tls_mode(raw: Option<&str>) -> Result<Option<&'static str>, ApiError> {
    let Some(raw) = raw.map(str::trim).filter(|m| !m.is_empty()) else {
        return Ok(None);
    };
    let lowered = raw.to_ascii_lowercase();
    TLS_MODES
        .iter()
        .find(|m| **m == lowered)
        .copied()
        .map(Some)
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "tls_mode must be one of none, acme or provided",
            )
        })
}

/// Everything the panel decides about a stack's TLS before touching the agent.
pub(crate) struct TlsPlan {
    pub(crate) mode: &'static str,
    /// The address that will be STORED, in every mode — an edit that says
    /// nothing about the address keeps it, and a stack switched to `none` and
    /// back to `acme` finds it again. What the agent is SENT is decided by
    /// `deploy_email`, which withholds it outside acme mode.
    ssl_email: Option<String>,
    /// The alias, provided mode only.
    pub(crate) alias: Option<String>,
    /// The registry row, provided mode only — resolved AND checked for coverage.
    certificate_id: Option<Uuid>,
}

impl TlsPlan {
    /// The address the AGENT is handed. Only an ACME order needs one, and an
    /// agent older than the stored mode still infers the mode FROM the address —
    /// so a stack whose stored mode is `none` or `provided` is sent none, or that
    /// agent would order the certificate the operator switched off.
    pub(crate) fn deploy_email(&self) -> Option<&str> {
        if self.mode == "acme" {
            self.ssl_email.as_deref()
        } else {
            None
        }
    }
}

/// Turn a mode, an address and an alias into a plan, refusing every combination
/// that cannot be served — before any row is written or any container touched.
///
/// Provided mode is the door that reaches the agent: the alias must name a row
/// on this server (400), the agent must be new enough to honour the mode at all
/// (412, fail-closed), and the certificate must actually cover the domain — the
/// agent's `cert_covers_domain` answers that, and its refusal passes through
/// unchanged. This is binding point 3 of #104: SAN validation at claim time.
///
/// `pub(crate)` since s414: reused by `routes::docker_apps::deploy` (template
/// apps) so a template deploy can request `provided` mode too — the agent's
/// own `/apps/deploy` has accepted `tls_mode`/`tls_certificate` since
/// `TlsIntent` was built, but nothing on the panel side ever sent them.
pub(crate) async fn plan_tls(
    db: &sqlx::PgPool,
    agent: &AgentHandle,
    mode: &'static str,
    domain: Option<&str>,
    ssl_email: Option<&str>,
    alias: Option<&str>,
    user_id: Uuid,
    server_id: Uuid,
) -> Result<TlsPlan, ApiError> {
    let ssl_email = ssl_email.map(str::trim).filter(|e| !e.is_empty()).map(str::to_string);
    if mode == "none" {
        return Ok(TlsPlan { mode, ssl_email, alias: None, certificate_id: None });
    }
    let Some(domain) = domain.map(str::trim).filter(|d| !d.is_empty()) else {
        return Err(err(StatusCode::BAD_REQUEST, "a TLS mode needs a domain"));
    };
    if mode == "acme" {
        if ssl_email.is_none() {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "Let's Encrypt mode needs an ssl_email for the ACME account",
            ));
        }
        return Ok(TlsPlan { mode, ssl_email, alias: None, certificate_id: None });
    }

    // provided
    let alias = alias
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            err(StatusCode::BAD_REQUEST, "provided mode needs the alias of a registered certificate")
        })?;
    if !is_valid_cert_alias(&alias) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Alias must be 1-64 lowercase letters, digits or hyphens, starting and ending with a letter or digit",
        ));
    }
    let certificate_id = certificate_id_for_alias(db, &alias, user_id, server_id).await?;
    require_agent_at_least(agent, PROVIDED_TLS_MIN_AGENT, "Serving a registered certificate")
        .await?;
    agent
        .post(
            &format!("/ssl/registry/{alias}/covers"),
            Some(serde_json::json!({ "domain": domain })),
        )
        .await
        .map_err(|e| agent_error("Certificate coverage check", e))?;

    Ok(TlsPlan { mode, ssl_email, alias: Some(alias), certificate_id: Some(certificate_id) })
}

/// Why a provided-mode deploy did not end in an HTTPS vhost, if it did not.
///
/// The agent never fails a deploy over its vhost: a refused or broken TLS leg
/// is a warning inside a 200 with the containers running. For Let's Encrypt
/// that is the right shape — the certificate can be ordered later. For a
/// registered certificate it is the outage this feature exists to prevent, so
/// the caller turns this into a 502 that names the cause.
fn provided_tls_refusal(deploy_result: &serde_json::Value) -> Option<String> {
    if deploy_result.get("ssl").and_then(|v| v.as_bool()) == Some(true) {
        return None;
    }
    // The agent puts the reason in `proxy_warning` and sets `tls_refused`
    // beside it as a FLAG — a boolean, never the sentence. Reading the flag as
    // the sentence answered every refusal with the fallback below.
    let sentence = deploy_result
        .get("proxy_warning")
        .and_then(|v| v.as_str())
        .or_else(|| deploy_result.get("tls_refused").and_then(|v| v.as_str()))
        .unwrap_or("the agent did not report an HTTPS vhost for the domain");
    Some(sentence.to_string())
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Stack {
    pub id: Uuid,
    pub user_id: Uuid,
    /// The host this stack's containers actually run on.
    ///
    /// Carried on the row because every agent call about a stack has to be aimed by it. A
    /// caller's `ServerScope` is a UI selection plus a local-agent fallback; it says which
    /// machine the operator is looking at, never which machine the stack is on.
    ///
    /// `NOT NULL` in the schema — `20260319000000_multi_server.sql` backfills every row and
    /// only then sets the constraint — so unlike `sites.server_id`, which predates the fleet
    /// work and stayed optional, this is a plain `Uuid`. It gets wrapped in `Some` at the one
    /// place it meets the shared helper, rather than the type carrying a state the column
    /// cannot hold.
    pub server_id: Uuid,
    pub name: String,
    pub yaml: String,
    pub service_count: i32,
    /// Domain this stack is served on, if the operator gave it one.
    pub domain: Option<String>,
    pub ssl_email: Option<String>,
    /// How the domain is served, as stored. NULL on a row an older binary wrote;
    /// read through `effective_tls_mode`, never directly.
    pub tls_mode: Option<String>,
    /// The registered certificate a provided-mode stack serves.
    pub tls_certificate_id: Option<Uuid>,
    /// That certificate's alias, joined in by `STACK_SELECT`. Defaulted so the
    /// create path's `INSERT … RETURNING`, which has no join to read it from,
    /// still maps onto this struct; the handler knows the alias it just stored.
    #[sqlx(default)]
    pub tls_certificate: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateStackRequest {
    pub name: String,
    pub yaml: String,
    /// Optional domain to front the stack with.
    pub domain: Option<String>,
    /// ACME address, acme mode only.
    pub ssl_email: Option<String>,
    /// "none" | "acme" | "provided". Absent = derived from the address, which is
    /// what every client before the mode existed meant by sending or not sending one.
    pub tls_mode: Option<String>,
    /// Registry alias, provided mode only.
    pub tls_certificate: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct UpdateStackRequest {
    pub yaml: String,
    /// Absent means KEEP the stored domain; an explicit null or blank vacates
    /// it. Before the mode was stored, an omitted domain silently tore the
    /// vhost down on an edit that was about something else.
    #[serde(default, deserialize_with = "super::secrets::explicit_option")]
    pub domain: Option<Option<String>>,
    /// ACME address. Absent means KEEP the stored one.
    ///
    /// It used to mean "clear": `update` forwarded whatever arrived here on every
    /// redeploy and never fell back to the address already stored, so omitting it
    /// for a stack that already HAD a certificate rewrote the vhost without its
    /// `:443` block, behind a year of HSTS. The mode is now a fact on the row, and
    /// an edit that says nothing about TLS changes nothing about TLS.
    pub ssl_email: Option<String>,
    /// Absent means keep the stored mode.
    pub tls_mode: Option<String>,
    /// Absent means keep the stored alias.
    pub tls_certificate: Option<String>,
}

/// Did anything in the stack actually come up?
///
/// The agent reports per-service outcomes inside a 200 — `deploy_compose`
/// returns a result, never an `Err` — so a caller that only checks the HTTP
/// status believes a stack deployed when none of it did.
pub(crate) fn deployed_service_states(
    deploy_result: &serde_json::Value,
) -> (usize, usize, Vec<String>) {
    let services = deploy_result
        .get("services")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let total = services.len();
    let running = services
        .iter()
        .filter(|s| s.get("status").and_then(|v| v.as_str()) == Some("running"))
        .count();
    let errors = services
        .iter()
        .filter_map(|s| {
            let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("service");
            s.get("error")
                .and_then(|v| v.as_str())
                .map(|e| format!("{name}: {e}"))
        })
        .collect();
    (running, total, errors)
}

/// Normalise + claim a stack's domain, or clear it.
async fn claim_stack_domain(
    state: &AppState,
    headers: &HeaderMap,
    domain: Option<&str>,
    holder: Holder,
    claimant_role: &str,
) -> Result<Option<String>, ApiError> {
    let Some(raw) = domain.map(str::trim).filter(|d| !d.is_empty()) else {
        return Ok(None);
    };
    let claimed =
        domain_claim::ensure_claimable(&state.db, &state.agents, raw, headers, holder, claimant_role)
            .await?;
    Ok(Some(claimed))
}

/// GET /api/stacks — List all stacks for the current user.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let stacks: Vec<Stack> = sqlx::query_as(&format!(
        "{STACK_SELECT} WHERE s.user_id = $1 AND s.server_id = $2 ORDER BY s.created_at DESC"
    ))
    .bind(claims.sub)
    .bind(server_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list stacks", e))?;

    // Get live container status from agent
    let apps = agent
        .get("/apps")
        .await
        .unwrap_or(serde_json::json!([]));

    let apps_arr = apps.as_array().cloned().unwrap_or_default();

    // Build response with live status per stack
    let result: Vec<serde_json::Value> = stacks
        .iter()
        .map(|stack| {
            let stack_id_str = stack.id.to_string();
            let services: Vec<&serde_json::Value> = apps_arr
                .iter()
                .filter(|a| a.get("stack_id").and_then(|v| v.as_str()) == Some(&stack_id_str))
                .collect();

            let running = services
                .iter()
                .filter(|a| a.get("status").and_then(|v| v.as_str()) == Some("running"))
                .count();
            let total = services.len();

            serde_json::json!({
                "id": stack.id,
                "name": stack.name,
                "service_count": stack.service_count,
                "domain": stack.domain,
                "ssl_email": stack.ssl_email,
                "tls_mode": effective_tls_mode(stack.tls_mode.as_deref(), stack.ssl_email.as_deref()),
                "tls_certificate": stack.tls_certificate,
                "running": running,
                "total": total,
                "status": if total == 0 { "removed" } else if running == total { "running" } else if running == 0 { "stopped" } else { "partial" },
                "services": services,
                "created_at": stack.created_at,
                "updated_at": stack.updated_at,
            })
        })
        .collect();

    Ok(Json(serde_json::json!(result)))
}

/// GET /api/stacks/{id} — Get stack details with live service status.
///
/// Takes no `ServerScope`. Unlike `list`, which is a per-server inventory and is meant to be
/// scoped by the caller, this reads one known row and must follow that row to its host.
pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let stack: Stack = sqlx::query_as(&format!("{STACK_SELECT} WHERE s.id = $1 AND s.user_id = $2"))
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("get_one stacks", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Stack not found"))?;

    // `/apps` is one machine's container inventory, and the filter below matches on the
    // `stack_id` label, which only exists on the host that deployed it. Asked of any other
    // host the list comes back with no match at all — and because a missing container is
    // indistinguishable here from a stopped one, the endpoint answered `running: 0,
    // services: []` for a stack that was up. That is the shape of an outage, arrived at
    // without one, and an operator who acts on it stops or removes a healthy stack.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(stack.server_id),
        stack.domain.as_deref().unwrap_or(&stack.name),
    )
    .await?;

    let apps = agent
        .get("/apps")
        .await
        .unwrap_or(serde_json::json!([]));

    let stack_id_str = stack.id.to_string();
    let services: Vec<&serde_json::Value> = apps
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|a| a.get("stack_id").and_then(|v| v.as_str()) == Some(&stack_id_str))
                .collect()
        })
        .unwrap_or_default();

    let running = services
        .iter()
        .filter(|a| a.get("status").and_then(|v| v.as_str()) == Some("running"))
        .count();

    Ok(Json(serde_json::json!({
        "id": stack.id,
        "name": stack.name,
        "yaml": stack.yaml,
        "service_count": stack.service_count,
        "domain": stack.domain,
        "ssl_email": stack.ssl_email,
        "tls_mode": effective_tls_mode(stack.tls_mode.as_deref(), stack.ssl_email.as_deref()),
        "tls_certificate": stack.tls_certificate,
        "running": running,
        "total": services.len(),
        "services": services,
        "created_at": stack.created_at,
        "updated_at": stack.updated_at,
    })))
}

/// POST /api/stacks — Create and deploy a new stack.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
    headers: HeaderMap,
    Json(body): Json<CreateStackRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    if body.name.trim().is_empty() || body.name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid stack name"));
    }
    if body.yaml.len() > 65536 {
        return Err(err(StatusCode::BAD_REQUEST, "YAML too large (max 64KB)"));
    }

    // The container-escape validator guarded `POST /api/apps/compose/deploy` and
    // nothing else, so the endpoint the UI actually posts to skipped it. The
    // agent re-rejects each of these itself, which is why it was never
    // exploitable — but a defence that only one of two doors performs is one
    // refactor away from being no defence.
    super::validate_compose_yaml(&body.yaml).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;

    // …and the same is true of the deploy gate: a stack runs images, so a CVE
    // threshold the operator set applies to them. Judged before the domain is
    // claimed and before any row is written, so a refusal leaves nothing behind.
    let images = super::compose_images(&body.yaml);
    crate::routes::docker_apps::enforce_allowed_images(&state.db, claims.sub, &images).await?;
    for image in images {
        crate::routes::image_scans::preflight_gate_image(&state.db, server_id, &agent, &image)
            .await?;
    }

    // The mode is decided here, once, and stored: a client that predates the
    // field is read the way the agent always read it, from the address.
    let mode = requested_tls_mode(body.tls_mode.as_deref())?
        .unwrap_or_else(|| effective_tls_mode(None, body.ssl_email.as_deref()));

    let domain = claim_stack_domain(
        &state,
        &headers,
        body.domain.as_deref(),
        Holder::New,
        &claims.role,
    )
    .await?;

    // Everything TLS is settled — including the agent's own answer to "does
    // this certificate cover this domain" — before a row exists or a container
    // is created, so a refusal leaves nothing to clean up.
    let tls = plan_tls(
        &state.db,
        &agent,
        mode,
        domain.as_deref(),
        body.ssl_email.as_deref(),
        body.tls_certificate.as_deref(),
        claims.sub,
        server_id,
    )
    .await?;

    // Parse to get service count
    let parsed = agent
        .post(
            "/apps/compose/parse",
            Some(serde_json::json!({ "yaml": body.yaml })),
        )
        .await
        .map_err(|e| agent_error("Compose parse", e))?;

    let service_count = parsed
        .as_array()
        .map(|a| a.len() as i32)
        .unwrap_or(0);

    // Create DB record first to get the stack ID
    let stack: Stack = sqlx::query_as(
        "INSERT INTO docker_stacks (user_id, server_id, name, yaml, service_count, domain, \
         ssl_email, tls_mode, tls_certificate_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
         RETURNING id, user_id, server_id, name, yaml, service_count, domain, ssl_email, \
         tls_mode, tls_certificate_id, created_at, updated_at",
    )
    .bind(claims.sub)
    .bind(server_id)
    .bind(&body.name)
    .bind(&body.yaml)
    .bind(service_count)
    .bind(&domain)
    .bind(&tls.ssl_email)
    .bind(tls.mode)
    .bind(tls.certificate_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate key") {
            err(StatusCode::CONFLICT, "A stack with this name already exists")
        } else {
            internal_error("create stacks", e)
        }
    })?;

    // Deploy with stack_id label
    let deploy_result = agent
        .post(
            "/apps/compose/deploy",
            Some(serde_json::json!({
                "yaml": body.yaml,
                "stack_id": stack.id.to_string(),
                "domain": domain,
                "ssl_email": tls.deploy_email(),
                "tls_mode": tls.mode,
                "tls_certificate": tls.alias,
            })),
        )
        .await
        .map_err(|e| {
            // Rollback DB record on deploy failure
            let db = state.db.clone();
            let stack_id = stack.id;
            tokio::spawn(async move {
                let _ = sqlx::query("DELETE FROM docker_stacks WHERE id = $1")
                    .bind(stack_id)
                    .execute(&db)
                    .await;
            });
            agent_error("Stack deploy", e)
        })?;

    // A stack where nothing came up is not a stack. Leaving the row behind gave
    // the operator a Compose Stacks entry with no containers and a second one
    // on every retry, and the domain claim would have outlived the deploy.
    let (running, total, errors) = deployed_service_states(&deploy_result);
    if total > 0 && running == 0 {
        let _ = sqlx::query("DELETE FROM docker_stacks WHERE id = $1")
            .bind(stack.id)
            .execute(&state.db)
            .await;
        return Err(err(
            StatusCode::BAD_GATEWAY,
            &format!(
                "No service in the stack stayed running, so it was not saved. {}",
                errors.join(" | ")
            ),
        ));
    }

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "stack.create",
        Some("stack"),
        Some(&stack.name),
        None,
        None,
    )
    .await;

    // The containers are up and the row is kept — but a registered certificate
    // that did not reach the vhost is not a warning, it is the domain serving
    // plain HTTP behind HSTS. Say so, with the agent's reason, and leave the
    // stack in place so the operator can fix the cause and redeploy.
    if tls.mode == "provided" {
        if let Some(reason) = provided_tls_refusal(&deploy_result) {
            return Err(err(
                StatusCode::BAD_GATEWAY,
                &format!(
                    "The stack {} is running, but its domain was not put behind the registered \
                     certificate: {reason}. Fix the cause and redeploy the stack.",
                    stack.name
                ),
            ));
        }
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": stack.id,
            "name": stack.name,
            "service_count": service_count,
            "domain": domain,
            "tls_mode": tls.mode,
            "tls_certificate": tls.alias,
            "running": running,
            "total": total,
            "deploy_result": deploy_result,
        })),
    ))
}

/// POST /api/stacks/{id}/start — Start all services in a stack.
///
/// No `ServerScope` on any of the three verbs below: `stack_action` resolves the host from
/// the row it authorises, so there is nothing left for the caller's selection to decide.
pub async fn start(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    stack_action(&state, &claims, id, "start").await
}

/// POST /api/stacks/{id}/stop — Stop all services in a stack.
pub async fn stop(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    stack_action(&state, &claims, id, "stop").await
}

/// POST /api/stacks/{id}/restart — Restart all services in a stack.
pub async fn restart(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    stack_action(&state, &claims, id, "restart").await
}

/// DELETE /api/stacks/{id} — Remove all services and delete the stack.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let stack: Option<(Uuid, String, Option<String>, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, name, domain, server_id FROM docker_stacks WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("remove stacks", e))?;

    let (_, name, domain, server_id) =
        stack.ok_or_else(|| err(StatusCode::NOT_FOUND, "Stack not found"))?;

    // This one deletes the row whatever the agent says (see below), so aiming it at the
    // wrong host was worse than the same mistake elsewhere: the removal would find nothing,
    // report success, drop the record, and leave the real host's containers and vhost
    // running with nothing in the database naming them. Resolve from the row and refuse if
    // that host is unreachable, so the record cannot outlive the thing it describes.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        server_id,
        domain.as_deref().unwrap_or(&name),
    )
    .await?;

    // Remove all containers, and the vhost/certs the stack was fronted by. The
    // agent proves ownership of each before deleting it.
    let result = agent
        .post(
            "/apps/stack/action",
            Some(serde_json::json!({
                "stack_id": id.to_string(),
                "action": "remove",
                "domain": domain,
            })),
        )
        .await;

    // Forget the expectation for every container the agent reports gone.
    //
    // The stack row is about to be deleted; the expectation is not tied to it.
    // It is keyed on (server_id, container_name) and would outlive the stack —
    // and the next container to claim that name would inherit the silence, with
    // nothing able to clear it, because a removed container is never observed
    // again. `stack_action` below already does exactly this for its own
    // `removed` results; this door was handed the same response and dropped it.
    if let (Some(server_id), Ok(body)) = (server_id, result.as_ref()) {
        if let Some(results) = body.get("results").and_then(|v| v.as_array()) {
            for r in results {
                if r.get("status").and_then(|v| v.as_str()) == Some("removed") {
                    if let Some(cname) = r.get("name").and_then(|v| v.as_str()) {
                        expected_stops::clear(&state.db, server_id, cname).await;
                    }
                }
            }
        }
    }

    // Delete DB record even if container removal had partial failures
    sqlx::query("DELETE FROM docker_stacks WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove stacks", e))?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "stack.remove",
        Some("stack"),
        Some(&name),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "name": name,
        "agent_result": result.ok(),
    })))
}

/// PUT /api/stacks/{id} — Update stack by removing old containers and redeploying.
///
/// The stored YAML is the only copy of a stack's definition — there is no
/// history table for `docker_stacks` the way there is for git deploys and
/// secrets. It used to be overwritten *after* the redeploy and unconditionally,
/// and since the agent reports per-service failure inside a 200, a redeploy
/// where every service failed still replaced the last-known-good file with the
/// YAML that had just failed. The old definition is now held until the new one
/// is known to run, and restored when it does not.
pub async fn update(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<UpdateStackRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    if body.yaml.len() > 65536 {
        return Err(err(StatusCode::BAD_REQUEST, "YAML too large (max 64KB)"));
    }
    super::validate_compose_yaml(&body.yaml).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;

    // Keep what we are about to replace — and read the host off the same row while we are
    // here. `name` comes along only so a refusal below can say which stack it refused.
    // The TLS columns come along because an edit that says nothing about TLS keeps
    // them, and the alias is read through the join because the request speaks aliases.
    let previous: Option<(
        String,
        Option<String>,
        Option<String>,
        String,
        Uuid,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT s.yaml, s.domain, s.ssl_email, s.name, s.server_id, s.tls_mode, c.alias \
         FROM docker_stacks s LEFT JOIN tls_certificates c ON c.id = s.tls_certificate_id \
         WHERE s.id = $1 AND s.user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("update stacks", e))?;

    let (
        previous_yaml,
        previous_domain,
        previous_ssl_email,
        name,
        stack_server_id,
        previous_tls_mode,
        previous_alias,
    ) = previous.ok_or_else(|| err(StatusCode::NOT_FOUND, "Stack not found"))?;
    let previous_mode =
        effective_tls_mode(previous_tls_mode.as_deref(), previous_ssl_email.as_deref());

    // Absent means KEEP, for all four. This is the fix the #104 thread
    // promised: an edit used to forward the request's address verbatim, so a
    // client that omitted it took the certificate off a stack that had one.
    let requested_mode = requested_tls_mode(body.tls_mode.as_deref())?;
    let requested_domain: Option<String> = match body.domain {
        None => previous_domain.clone(),
        Some(explicit) => explicit,
    };
    let ssl_email = body
        .ssl_email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .or(previous_ssl_email.clone());
    let alias = body
        .tls_certificate
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .or(previous_alias.clone());

    // Everything from here down — the claim, the parse, the teardown, the redeploy and the
    // restore-on-failure — is one machine's work, and the row says which machine. Taking it
    // from the caller's request instead let an edit tear the stack down on one host and
    // stand it up on another.
    //
    // That is worse than a misdirected action, because it desynchronises the record from the
    // thing it describes and nothing afterwards notices. The row keeps naming the old host
    // while the containers, vhost and certificate now live on the new one. `remove` resolves
    // correctly from the row, so the next delete goes to the old host, finds nothing, reports
    // success, and drops the record — leaving the real containers running on a machine no
    // handler can reach any more, holding a domain and a certificate nothing will renew.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(stack_server_id),
        previous_domain.as_deref().unwrap_or(&name),
    )
    .await?;

    // An edit can introduce images the stack was not running before, so it is a
    // deploy as far as the gate is concerned. Judged against the stack's OWN
    // host — the same row that decided which agent to talk to — and before the
    // teardown below, so a refusal leaves the running stack untouched.
    let images = super::compose_images(&body.yaml);
    crate::routes::docker_apps::enforce_allowed_images(&state.db, claims.sub, &images).await?;
    for image in images {
        crate::routes::image_scans::preflight_gate_image(
            &state.db,
            stack_server_id,
            &agent,
            &image,
        )
        .await?;
    }

    // `Holder::Stack(id)` so a stack keeps its own domain across an edit;
    // anything else already holding it is still a conflict.
    //
    // The server id passed here is the stack's, not the caller's, and that matters
    // independently of which agent runs the deploy: `ensure_claimable` asks a specific host
    // whether the name is free *there*, and domains are unique per server. Asking the caller's
    // host answered a question about a different box — waving through a name already serving
    // on the real one, or refusing a name that was free.
    let domain = claim_stack_domain(
        &state,
        &headers,
        requested_domain.as_deref(),
        Holder::Stack(id),
        &claims.role,
    )
    .await?;

    // A stack with no domain has no vhost, so no TLS mode applies to it: a
    // vacated domain resolves to `none` unless the request names a mode, in
    // which case the plan below refuses the combination in words.
    let mode = match (requested_mode, domain.is_some()) {
        (Some(requested), _) => requested,
        (None, true) => previous_mode,
        (None, false) => "none",
    };

    // Same gate and coverage check as create, against the stack's own host, and
    // BEFORE the teardown below — a refusal must leave the running stack alone.
    let tls = plan_tls(
        &state.db,
        &agent,
        mode,
        domain.as_deref(),
        ssl_email.as_deref(),
        alias.as_deref(),
        claims.sub,
        stack_server_id,
    )
    .await?;

    // Parse new YAML
    let parsed = agent
        .post(
            "/apps/compose/parse",
            Some(serde_json::json!({ "yaml": body.yaml })),
        )
        .await
        .map_err(|e| agent_error("Compose parse", e))?;

    let service_count = parsed.as_array().map(|a| a.len() as i32).unwrap_or(0);

    // Remove old containers. The vhost only comes down when the domain is
    // actually changing — tearing it down on every edit would drop the site for
    // as long as the redeploy takes.
    let vacating = previous_domain.as_deref().filter(|d| Some(*d) != domain.as_deref());
    let removed = agent
        .post(
            "/apps/stack/action",
            Some(serde_json::json!({
                "stack_id": id.to_string(),
                "action": "remove",
                "domain": vacating,
            })),
        )
        .await;

    // These come straight back under the SAME names, which is why the engine's
    // absence sweep cannot help here: the container is gone and replaced inside
    // one 120-second tick, so it is never observed missing. If a replacement
    // fails to start it would inherit the old container's expectation and be
    // silenced — the one shape this door can produce and the sweep cannot see.
    if let Ok(ref body) = removed {
        if let Some(results) = body.get("results").and_then(|v| v.as_array()) {
            for r in results {
                if r.get("status").and_then(|v| v.as_str()) == Some("removed") {
                    if let Some(cname) = r.get("name").and_then(|v| v.as_str()) {
                        expected_stops::clear(&state.db, stack_server_id, cname).await;
                    }
                }
            }
        }
    }

    // Deploy new containers with same stack_id
    let deploy_result = agent
        .post(
            "/apps/compose/deploy",
            Some(serde_json::json!({
                "yaml": body.yaml,
                "stack_id": id.to_string(),
                "domain": domain,
                "ssl_email": tls.deploy_email(),
                "tls_mode": tls.mode,
                "tls_certificate": tls.alias,
            })),
        )
        .await;

    let deploy_result = match deploy_result {
        Ok(r) => r,
        Err(e) => {
            restore_previous(
                &state.db,
                stack_server_id,
                &agent,
                id,
                &previous_yaml,
                previous_domain.as_deref(),
                previous_ssl_email.as_deref(),
                previous_mode,
                previous_alias.as_deref(),
            )
            .await;
            return Err(agent_error("Stack redeploy (previous definition restored)", e));
        }
    };

    let (running, total, errors) = deployed_service_states(&deploy_result);
    if total > 0 && running == 0 {
        restore_previous(
            &state.db,
            stack_server_id,
            &agent,
            id,
            &previous_yaml,
            previous_domain.as_deref(),
            previous_ssl_email.as_deref(),
            previous_mode,
            previous_alias.as_deref(),
        )
        .await;
        return Err(err(
            StatusCode::BAD_GATEWAY,
            &format!(
                "No service in the new definition stayed running — the previous stack was \
                 redeployed and the saved YAML is unchanged. {}",
                errors.join(" | ")
            ),
        ));
    }

    // Only now is the new definition the one worth keeping. The TLS columns are
    // written from the plan, which already folded "absent means keep" in Rust —
    // no COALESCE, so the statement says exactly what the row will hold.
    // `ssl_expiry` describes the certificate serving THIS ROW'S DOMAIN in ACME
    // mode. Both of those can change here, and when either does the recorded date
    // stops being about anything this row still owns — a stack moved to a new
    // domain would keep publishing the old domain's countdown, and one switched to
    // `provided` or `none` would keep publishing a date for a certificate the
    // panel no longer renews. NULL is this column's word for "not recorded", so
    // that is what it becomes; the next read of the host's certificates puts a
    // true value back.
    //
    // ⛔ Kept, deliberately, when neither changed. Blanking on every edit would
    // make an ordinary YAML change erase the one answer the offline view has.
    // `vacating` is already the file's spelling of "the domain moved" — reusing it
    // keeps one definition rather than a second that can drift from it.
    let keeps_expiry = tls.mode == "acme" && vacating.is_none();

    sqlx::query(
        "UPDATE docker_stacks SET yaml = $1, service_count = $2, domain = $3, ssl_email = $4, \
         tls_mode = $5, tls_certificate_id = $6, \
         ssl_expiry = CASE WHEN $8 THEN ssl_expiry ELSE NULL END, \
         updated_at = NOW() WHERE id = $7",
    )
    .bind(&body.yaml)
    .bind(service_count)
    .bind(&domain)
    .bind(&tls.ssl_email)
    .bind(tls.mode)
    .bind(tls.certificate_id)
    .bind(id)
    .bind(keeps_expiry)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("update stacks", e))?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "stack.update",
        Some("stack"),
        Some(&name),
        None,
        None,
    )
    .await;

    // Same as create: the definition runs and is saved, but a provided-mode
    // domain that came back without its HTTPS vhost is an outage, not a note.
    if tls.mode == "provided" {
        if let Some(reason) = provided_tls_refusal(&deploy_result) {
            return Err(err(
                StatusCode::BAD_GATEWAY,
                &format!(
                    "The stack {name} is running with the new definition, but its domain was \
                     not put behind the registered certificate: {reason}. Fix the cause and \
                     redeploy the stack."
                ),
            ));
        }
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "service_count": service_count,
        "domain": domain,
        "tls_mode": tls.mode,
        "tls_certificate": tls.alias,
        "running": running,
        "total": total,
        "deploy_result": deploy_result,
    })))
}

/// Put the stack back the way it was after a failed edit.
///
/// Best effort — if this also fails the operator still has the stored YAML,
/// which is the whole point of not having overwritten it yet.
///
/// Carries the previous TLS mode and alias too: a restore that sent only the
/// address would put a provided-certificate stack back as plain HTTP.
#[allow(clippy::too_many_arguments)]
async fn restore_previous(
    pool: &sqlx::PgPool,
    server_id: Uuid,
    agent: &AgentHandle,
    id: Uuid,
    yaml: &str,
    domain: Option<&str>,
    ssl_email: Option<&str>,
    tls_mode: &str,
    tls_certificate: Option<&str>,
) {
    let removed = agent
        .post(
            "/apps/stack/action",
            Some(serde_json::json!({ "stack_id": id.to_string(), "action": "remove" })),
        )
        .await;

    // Same reasoning as the vacate leg above: removed and redeployed under the
    // same names inside one sweep, so nothing else can ever clear these.
    if let Ok(ref body) = removed {
        if let Some(results) = body.get("results").and_then(|v| v.as_array()) {
            for r in results {
                if r.get("status").and_then(|v| v.as_str()) == Some("removed") {
                    if let Some(cname) = r.get("name").and_then(|v| v.as_str()) {
                        expected_stops::clear(pool, server_id, cname).await;
                    }
                }
            }
        }
    }
    match agent
        .post(
            "/apps/compose/deploy",
            Some(serde_json::json!({
                "yaml": yaml,
                "stack_id": id.to_string(),
                "domain": domain,
                "ssl_email": if tls_mode == "acme" { ssl_email } else { None },
                "tls_mode": tls_mode,
                "tls_certificate": tls_certificate,
            })),
        )
        .await
    {
        Ok(_) => tracing::info!("Stack {id}: restored the previous definition after a failed update"),
        Err(e) => tracing::error!(
            "Stack {id}: the update failed AND the previous definition could not be \
             redeployed ({e}). The saved YAML is still the previous one."
        ),
    }
}

/// Internal helper for start/stop/restart stack actions.
///
/// Takes no agent, deliberately. This is the shared seam for all three verbs, and the
/// ownership check it already performs is the same lookup that names the host — so asking
/// the row one more column makes the guard and the resolver a single query, and no caller
/// can supply a handle that disagrees with what was just authorised. Patching the three
/// call sites instead would have left the next action verb free to reintroduce the bug.
///
/// What it was: the ownership check reads `id` and `user_id` and nothing else, so it proved
/// the caller may act on the stack and said nothing about where the stack is. The action
/// then went to the header-named host. `/apps/stack/action` matches containers by the
/// `stack_id` label, which exists only on the host that deployed them, so on the wrong host
/// a stop or restart matched nothing and returned a perfectly successful 200 — the operator
/// sees the verb succeed while the containers keep running untouched somewhere else.
async fn stack_action(
    state: &AppState,
    claims: &Claims,
    id: Uuid,
    action: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    // Verify ownership, and take the host from the same row that grants it.
    let owned: Option<(String, Option<String>, Uuid)> = sqlx::query_as(
        "SELECT name, domain, server_id FROM docker_stacks WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("stack action", e))?;

    let (name, domain, server_id) =
        owned.ok_or_else(|| err(StatusCode::NOT_FOUND, "Stack not found"))?;

    let agent = crate::helpers::agent_for_site_server(
        state,
        Some(server_id),
        domain.as_deref().unwrap_or(&name),
    )
    .await?;

    let result = agent
        .post(
            "/apps/stack/action",
            Some(serde_json::json!({
                "stack_id": id.to_string(),
                "action": action,
            })),
        )
        .await
        .map_err(|e| agent_error(&format!("Stack {action}"), e))?;

    // ── Stopping a stack is a deliberate stop, N times over ────────────────
    //
    // Compose services carry `dockpanel.managed=true` and
    // `dockpanel.app.template=compose`, which is exactly what the agent's
    // `/apps` listing filters on — so every service in a stopped stack is a
    // full `container_down` subject. One click on a five-service stack produced
    // five criticals, correlated into ONE public incident titled after whichever
    // container lost the race.
    //
    // The agent answers with a per-container `results[]` carrying the name and
    // the outcome, so the expectation is recorded per container and only for the
    // ones that actually stopped — no second round trip, and a service that
    // failed to stop is still allowed to alert.
    if let Some(results) = result.get("results").and_then(|v| v.as_array()) {
        for r in results {
            let Some(cname) = r.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            match r.get("status").and_then(|v| v.as_str()) {
                Some("stopped") => {
                    expected_stops::record(
                        &state.db,
                        server_id,
                        cname,
                        expected_stops::REASON_STACK_STOP,
                        Some(&claims.email),
                    )
                    .await;
                    expected_stops::resolve_open_container_down(&state.db, server_id, cname).await;
                }
                // `removed` clears too: the container is gone, and a row left
                // behind would suppress a future container that reuses the name.
                Some("started") | Some("restarted") | Some("removed") => {
                    expected_stops::clear(&state.db, server_id, cname).await;
                }
                _ => {}
            }
        }
    }

    Ok(Json(result))
}


/// POST /api/stacks/{id}/renew-ssl — reissue a Compose stack's ACME certificate
/// on the operator's say-so.
///
/// The Certificates page has always shown a stack's certificate and never been
/// able to do anything about it: both of its controls address a SITE, and a stack
/// has no `sites` row, so the row rendered "Not managed here" over a certificate
/// this product had issued and — since v2.161.0 — renews on a schedule. That
/// sentence was wrong twice over, and this is the control that makes the page
/// able to say something true.
///
/// A SIBLING of the scanner's `renew_stack_certificate`, deliberately not an
/// extraction. They differ in what they owe the caller, not just in who calls:
/// the scanner writes the activity row that IS its own cooldown and fires and
/// clears an alert; this one owes a REFUSAL WITH A SENTENCE, which is the entire
/// reason an operator would press a button rather than wait a week.
pub async fn renew_ssl(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let stack: Stack = sqlx::query_as(&format!("{STACK_SELECT} WHERE s.id = $1"))
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("stack renew ssl", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Stack not found"))?;

    let Some(domain) = stack.domain.as_deref().map(str::trim).filter(|d| !d.is_empty()) else {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "This stack has no domain, so there is no certificate to renew.",
        ));
    };

    // ⛔ Dispatch through the row's OWN server. A domain is unique only per
    //    server; renewing through whichever host the caller happened to be
    //    looking at would write a certificate on the wrong machine.
    let agent =
        crate::helpers::agent_for_site_server(&state, Some(stack.server_id), domain).await?;

    // The mode, through the one spelling of the rule.
    let mode = effective_tls_mode(stack.tls_mode.as_deref(), stack.ssl_email.as_deref());
    if mode != "acme" {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "{domain} is served in '{mode}' mode, not ACME. DockPanel reissues only the \
                 Let's Encrypt certificates it obtains itself — a registered certificate is \
                 replaced from the Certificates page, and a stack with no TLS has nothing to \
                 renew."
            ),
        ));
    }

    let Some(contact) = stack.ssl_email.as_deref().map(str::trim).filter(|e| !e.is_empty()) else {
        return Err(err(
            StatusCode::PRECONDITION_FAILED,
            "This stack is in ACME mode with no contact address, and a certificate authority \
             will not accept an order without one. Add an address to the stack first.",
        ));
    };

    // ⛔ THE ISSUER GUARD. The mode above is a DATABASE COLUMN; the thing about to
    //    be overwritten is a FILE, and the column knows nothing about it. A
    //    purchased certificate reaches that path by more than one route, and the
    //    registry migration backfilled every stack carrying an address to `acme`,
    //    so the mode guard waves all of them through.
    //
    //    `None` means "not proven foreign" and MUST proceed: refusing because an
    //    agent hiccuped would let a real certificate lapse.
    if let Some(issuer) = crate::helpers::foreign_cert_issuer(&agent, domain).await {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "The certificate on {domain} was not issued by DockPanel (issuer: {issuer}). \
                 Reissuing would replace it with a Let's Encrypt certificate. Renew it wherever \
                 it was issued, or install a replacement under Certificates."
            ),
        ));
    }

    // The agent must be new enough to accept a renewal that describes no vhost.
    // FROZEN at the release that taught it that; it names a capability, not the
    // current version.
    require_agent_at_least(
        &agent,
        super::tls_certificates::STACK_RENEWAL_MIN_AGENT,
        "Renewing a Compose stack's certificate",
    )
    .await?;

    // ⛔ NO `runtime` KEY. Its absence is the contract with the agent's in-place
    //    branch: the panel cannot describe a stack's vhost (only the agent knows
    //    the published port, which it derives from the compose file), and the
    //    certificate paths do not move, so the agent reloads rather than
    //    re-rendering. Sending one would publish a proxy vhost with no upstream
    //    and take the stack off the air to install a certificate.
    let result = agent
        .post_long(
            &format!("/ssl/{domain}/renew"),
            Some(serde_json::json!({ "email": contact })),
            crate::routes::ssl::DNS01_ORDER_TIMEOUT_SECS,
        )
        .await
        .map_err(|e| agent_error("SSL renewal", e))?;

    // Same parse as every other door: the agent prints a `time` crate Display,
    // and the column is TIMESTAMPTZ.
    let expiry = result
        .get("expiry")
        .and_then(|x: &serde_json::Value| x.as_str())
        .and_then(crate::helpers::parse_agent_cert_expiry);
    if let Some(exp) = expiry {
        let _ = sqlx::query("UPDATE docker_stacks SET ssl_expiry = $1 WHERE id = $2")
            .bind(exp)
            .bind(id)
            .execute(&state.db)
            .await;
    }

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "stack.renew_ssl",
        Some("stack"),
        Some(domain),
        Some(&format!("stack_id={id}")),
        None,
    )
    .await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "domain": domain,
        "expiry": expiry,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stored value wins; a NULL row (older binary, rollback) is read the way
    /// the agent always inferred it — a non-blank address means Let's Encrypt.
    #[test]
    fn effective_tls_mode_prefers_the_stored_value() {
        assert_eq!(effective_tls_mode(Some("none"), Some("ops@example.com")), "none");
        assert_eq!(effective_tls_mode(Some("acme"), None), "acme");
        assert_eq!(effective_tls_mode(Some("provided"), Some("ops@example.com")), "provided");
    }

    #[test]
    fn effective_tls_mode_derives_a_null_row_from_the_address() {
        assert_eq!(effective_tls_mode(None, Some("ops@example.com")), "acme");
        assert_eq!(effective_tls_mode(None, Some("  ")), "none");
        assert_eq!(effective_tls_mode(None, Some("")), "none");
        assert_eq!(effective_tls_mode(None, None), "none");
    }

    /// A value outside the vocabulary cannot reach the column (CHECK), but the
    /// reader must not panic or invent a fourth mode if one ever does.
    #[test]
    fn effective_tls_mode_falls_back_on_an_unknown_word() {
        assert_eq!(effective_tls_mode(Some("bogus"), Some("ops@example.com")), "acme");
        assert_eq!(effective_tls_mode(Some("bogus"), None), "none");
    }

    #[test]
    fn requested_tls_mode_normalises_and_refuses() {
        assert_eq!(requested_tls_mode(None).unwrap(), None);
        assert_eq!(requested_tls_mode(Some("")).unwrap(), None);
        assert_eq!(requested_tls_mode(Some(" Acme ")).unwrap(), Some("acme"));
        assert_eq!(requested_tls_mode(Some("provided")).unwrap(), Some("provided"));
        assert!(requested_tls_mode(Some("letsencrypt")).is_err());
    }

    /// The agent reports a refused TLS leg inside a 200; only an explicit
    /// `ssl: true` counts as the domain being served over HTTPS.
    #[test]
    fn provided_tls_refusal_reads_the_agent_answer() {
        assert_eq!(provided_tls_refusal(&serde_json::json!({ "ssl": true })), None);
        // The agent's real shape: the flag is a boolean, the sentence is the warning.
        assert_eq!(
            provided_tls_refusal(&serde_json::json!({
                "ssl": false, "tls_refused": true, "proxy_warning": "no such alias"
            })),
            Some("no such alias".to_string())
        );
        assert_eq!(
            provided_tls_refusal(&serde_json::json!({ "proxy_warning": "Traefik" })),
            Some("Traefik".to_string())
        );
        assert!(provided_tls_refusal(&serde_json::json!({})).is_some());
    }
}
