use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, Claims, ServerScope};
use crate::error::{internal_error, err, agent_error, require_admin, ApiError};
use crate::services::activity;
use crate::services::agent::AgentHandle;
use crate::services::domain_claim::{self, Holder};
use crate::AppState;

const STACK_SELECT: &str = "SELECT id, user_id, server_id, name, yaml, service_count, domain, \
                            ssl_email, created_at, updated_at FROM docker_stacks";

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
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateStackRequest {
    pub name: String,
    pub yaml: String,
    /// Optional domain to front the stack with.
    pub domain: Option<String>,
    /// ACME address. A domain with no address gets a vhost but no certificate.
    pub ssl_email: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct UpdateStackRequest {
    pub yaml: String,
    pub domain: Option<String>,
    pub ssl_email: Option<String>,
}

/// Did anything in the stack actually come up?
///
/// The agent reports per-service outcomes inside a 200 — `deploy_compose`
/// returns a result, never an `Err` — so a caller that only checks the HTTP
/// status believes a stack deployed when none of it did.
fn deployed_service_states(deploy_result: &serde_json::Value) -> (usize, usize, Vec<String>) {
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
        "{STACK_SELECT} WHERE user_id = $1 AND server_id = $2 ORDER BY created_at DESC"
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

    let stack: Stack = sqlx::query_as(&format!("{STACK_SELECT} WHERE id = $1 AND user_id = $2"))
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

    let domain = claim_stack_domain(
        &state,
        &headers,
        body.domain.as_deref(),
        Holder::New,
        &claims.role,
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
        "INSERT INTO docker_stacks (user_id, server_id, name, yaml, service_count, domain, ssl_email) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING id, user_id, server_id, name, yaml, service_count, domain, ssl_email, \
         created_at, updated_at",
    )
    .bind(claims.sub)
    .bind(server_id)
    .bind(&body.name)
    .bind(&body.yaml)
    .bind(service_count)
    .bind(&domain)
    .bind(&body.ssl_email)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create stacks", e))?;

    // Deploy with stack_id label
    let deploy_result = agent
        .post(
            "/apps/compose/deploy",
            Some(serde_json::json!({
                "yaml": body.yaml,
                "stack_id": stack.id.to_string(),
                "domain": domain,
                "ssl_email": body.ssl_email,
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

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": stack.id,
            "name": stack.name,
            "service_count": service_count,
            "domain": domain,
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
    let previous: Option<(String, Option<String>, Option<String>, String, Uuid)> = sqlx::query_as(
        "SELECT yaml, domain, ssl_email, name, server_id FROM docker_stacks \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("update stacks", e))?;

    let (previous_yaml, previous_domain, previous_ssl_email, name, stack_server_id) =
        previous.ok_or_else(|| err(StatusCode::NOT_FOUND, "Stack not found"))?;

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
        body.domain.as_deref(),
        Holder::Stack(id),
        &claims.role,
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
    let _ = agent
        .post(
            "/apps/stack/action",
            Some(serde_json::json!({
                "stack_id": id.to_string(),
                "action": "remove",
                "domain": vacating,
            })),
        )
        .await;

    // Deploy new containers with same stack_id
    let deploy_result = agent
        .post(
            "/apps/compose/deploy",
            Some(serde_json::json!({
                "yaml": body.yaml,
                "stack_id": id.to_string(),
                "domain": domain,
                "ssl_email": body.ssl_email,
            })),
        )
        .await;

    let deploy_result = match deploy_result {
        Ok(r) => r,
        Err(e) => {
            restore_previous(&agent, id, &previous_yaml, previous_domain.as_deref(), previous_ssl_email.as_deref()).await;
            return Err(agent_error("Stack redeploy (previous definition restored)", e));
        }
    };

    let (running, total, errors) = deployed_service_states(&deploy_result);
    if total > 0 && running == 0 {
        restore_previous(&agent, id, &previous_yaml, previous_domain.as_deref(), previous_ssl_email.as_deref()).await;
        return Err(err(
            StatusCode::BAD_GATEWAY,
            &format!(
                "No service in the new definition stayed running — the previous stack was \
                 redeployed and the saved YAML is unchanged. {}",
                errors.join(" | ")
            ),
        ));
    }

    // Only now is the new definition the one worth keeping.
    sqlx::query(
        "UPDATE docker_stacks SET yaml = $1, service_count = $2, domain = $3, ssl_email = $4, \
         updated_at = NOW() WHERE id = $5",
    )
    .bind(&body.yaml)
    .bind(service_count)
    .bind(&domain)
    .bind(&body.ssl_email)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("update stacks", e))?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "service_count": service_count,
        "domain": domain,
        "running": running,
        "total": total,
        "deploy_result": deploy_result,
    })))
}

/// Put the stack back the way it was after a failed edit.
///
/// Best effort — if this also fails the operator still has the stored YAML,
/// which is the whole point of not having overwritten it yet.
async fn restore_previous(
    agent: &AgentHandle,
    id: Uuid,
    yaml: &str,
    domain: Option<&str>,
    ssl_email: Option<&str>,
) {
    let _ = agent
        .post(
            "/apps/stack/action",
            Some(serde_json::json!({ "stack_id": id.to_string(), "action": "remove" })),
        )
        .await;
    match agent
        .post(
            "/apps/compose/deploy",
            Some(serde_json::json!({
                "yaml": yaml,
                "stack_id": id.to_string(),
                "domain": domain,
                "ssl_email": ssl_email,
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

    Ok(Json(result))
}
