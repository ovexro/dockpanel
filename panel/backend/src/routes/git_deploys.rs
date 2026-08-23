use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures::stream::StreamExt;
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, require_admin, ApiError};
use crate::routes::{is_valid_name, is_reserved_domain};
use crate::routes::sites::ProvisionStep;
use crate::services::activity;
use crate::services::agent::AgentHandle;
use crate::services::domain_claim;
use crate::services::notifications;
use crate::AppState;

/// git_previews.container_name is stored WITH the `dockpanel-git-` prefix that
/// the agent's `/git/cleanup` handler re-adds (agent `cleanup_container` does
/// `format!("dockpanel-git-{name}")`). Passing the stored value verbatim yields
/// a double-prefixed name that matches no container, so the teardown silently
/// no-ops and leaks the container/image/nginx/SSL/repo dir while the DB frees
/// its port. Every preview-cleanup caller MUST route the stored name through
/// this so the agent resolves the real container (mirrors the manual
/// `delete_preview` path). See lesson #70 (resolve by the identity the resource
/// was created with, and fix the whole class at one choke point).
pub(crate) fn strip_container_prefix(stored: &str) -> &str {
    stored.strip_prefix("dockpanel-git-").unwrap_or(stored)
}

/// The container-name prefix a preview is created under from v2.55.0 on.
///
/// `.` is the whole point: `is_valid_name` rejects it, so no *deployment* can be
/// named into this space. Before the split, a preview of config `C` on branch
/// `B` was `dockpanel-git-{C}-pr-{slug(B)}` — which is exactly the container of
/// a deployment named `{C}-pr-{slug(B)}`, a name the panel will happily create.
pub(crate) const PREVIEW_SCOPE_PREFIX: &str = "pr.";

/// Turn a stored `git_previews.container_name` into the `(name, scope)` pair the
/// agent needs to address it.
///
/// Rows written before the split carry the old unscoped name and must be torn
/// down in the old space — hence `preview_legacy`, which addresses that name but
/// refuses any container whose labels say it is a deployment. Recomputing the
/// name from `{config}-pr-{slug(branch)}` instead would be a second answer to a
/// question the row already answers, and would drift the day `dns_label` does.
pub(crate) fn preview_cleanup_target(stored: &str) -> (String, &'static str) {
    let bare = strip_container_prefix(stored);
    match bare.strip_prefix(PREVIEW_SCOPE_PREFIX) {
        Some(name) => (name.to_string(), "preview"),
        None => (bare.to_string(), "preview_legacy"),
    }
}

/// The one body every preview teardown sends to `POST /git/cleanup`.
///
/// There are five call sites — the TTL sweep, the stuck sweep, the manual
/// delete, the unauthenticated branch-delete webhook, and the parent
/// deployment's own removal — and they had drifted into three different
/// spellings, one of which double-prefixed the name into a no-op. `domain` and
/// `host_port` come from the row because the agent reads them off the container,
/// which is exactly what is missing when a crashed preview is being swept.
pub(crate) fn preview_cleanup_body(
    container_name: &str,
    domain: Option<&str>,
    host_port: Option<i32>,
) -> serde_json::Value {
    let (name, scope) = preview_cleanup_target(container_name);
    let mut body = serde_json::json!({ "name": name, "scope": scope });
    if let Some(d) = domain {
        if !d.is_empty() {
            body["domain"] = serde_json::json!(d);
        }
    }
    if let Some(p) = host_port {
        if (0..=u16::MAX as i32).contains(&p) {
            body["host_port"] = serde_json::json!(p);
        }
    }
    body
}

/// Convert an arbitrary branch name into a DNS-label-safe slug for the preview
/// subdomain / container name: lowercase, every run of chars outside `[a-z0-9]`
/// collapses to a single '-', with leading/trailing '-' trimmed. Keeps the
/// synthesized preview domain (`{slug}.{base}`) a valid hostname so the agent's
/// `is_valid_domain` check accepts it — an underscore branch like
/// `feature_login` would otherwise fail domain validation and the preview would
/// silently fail. Falls back to "preview" if nothing alphanumeric remains.
pub(crate) fn dns_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let slug = out.trim_matches('-').to_string();
    if slug.is_empty() { "preview".to_string() } else { slug }
}

/// Validate a single crontab field against the exact surface the executor
/// (`deploy_scheduler::matches_field`) accepts: a comma list whose parts are each
/// `*`, `*/N` (N>0), an `a-b` range, or a plain number. Format-only — the goal is
/// to reject unparseable garbage WITHOUT being stricter than the scheduler that
/// runs it (a validator stricter than the executor rejects working crons on edit).
fn is_valid_cron_field(field: &str) -> bool {
    !field.is_empty()
        && field.split(',').all(|part| {
            part.is_empty() // scheduler skips empty comma segments (e.g. "1,,5"); don't be stricter
                || part == "*"
                || part
                    .strip_prefix("*/")
                    .map(|n| n.parse::<u32>().map(|v| v > 0).unwrap_or(false))
                    .unwrap_or(false)
                || part
                    .split_once('-')
                    .map(|(a, b)| a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok())
                    .unwrap_or(false)
                || part.parse::<u32>().is_ok()
        })
}

/// A `deploy_cron` value is valid iff it has AT LEAST 5 whitespace-separated
/// fields (the scheduler reads the first 5 and ignores extras — a 6-field
/// "seconds" cron still fires) and each of those first 5 is individually valid.
pub(crate) fn is_valid_cron(expr: &str) -> bool {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    parts.len() >= 5 && parts[..5].iter().all(|f| is_valid_cron_field(f))
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct GitDeploy {
    pub id: Uuid,
    pub user_id: Uuid,
    pub server_id: Uuid,
    pub name: String,
    pub repo_url: String,
    pub branch: String,
    pub dockerfile: String,
    pub container_port: i32,
    pub host_port: i32,
    pub domain: Option<String>,
    pub env_vars: serde_json::Value,
    pub auto_deploy: bool,
    pub webhook_secret: String,
    pub deploy_key_public: Option<String>,
    pub deploy_key_path: Option<String>,
    pub container_id: Option<String>,
    pub image_tag: Option<String>,
    pub status: String,
    pub memory_mb: Option<i32>,
    pub cpu_percent: Option<i32>,
    pub ssl_email: Option<String>,
    pub pre_build_cmd: Option<String>,
    pub post_deploy_cmd: Option<String>,
    pub build_args: serde_json::Value,
    pub build_context: String,
    pub last_deploy: Option<chrono::DateTime<chrono::Utc>>,
    pub last_commit: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub github_token: Option<String>,
    pub deploy_cron: Option<String>,
    pub deploy_protected: bool,
    pub build_method: String,
    pub preview_ttl_hours: i32,
    pub scheduled_deploy_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct GitPreview {
    pub id: Uuid,
    pub git_deploy_id: Uuid,
    pub branch: String,
    pub container_name: String,
    pub container_id: Option<String>,
    pub host_port: i32,
    pub domain: Option<String>,
    pub status: String,
    pub commit_hash: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Everything `POST /git/deploy` carries. Named as one struct so that adding a
/// field to the agent's `DeployRequest` forces every caller here to say what it
/// passes for it, rather than silently passing nothing.
pub(crate) struct DeployBody<'a> {
    pub name: &'a str,
    pub image_tag: &'a str,
    pub container_port: i32,
    pub host_port: i32,
    pub env_vars: &'a serde_json::Value,
    pub domain: Option<&'a str>,
    pub memory_mb: Option<i32>,
    pub cpu_percent: Option<i32>,
    pub ssl_email: Option<&'a str>,
    /// Which of the agent's two name spaces this deploy addresses — `"deploy"`
    /// or `"preview"`. Not defaulted on purpose: an omitted scope is precisely
    /// how a preview came to be able to name a deployment's container.
    pub scope: &'a str,
}

/// Build the body for the agent's `POST /git/deploy`.
///
/// This exists because five call sites used to assemble the same object by hand
/// and drifted apart in two directions at once (GH #94):
///
///   * Four of them sent the environment under the key `env_vars`, after the
///     column it is loaded from. The agent's handler declares that field as `env`
///     with `#[serde(default)]`, so the unrecognised key was dropped and the
///     recognised one defaulted to empty. Every git deploy — Dockerfile AND
///     Nixpacks, first deploy and redeploy — started its container with no
///     environment, and no layer reported a problem: the panel logged a success
///     and the agent logged a deploy. Only `docker inspect` disagreed. Nixpacks
///     appeared to work because `/git/nixpacks-build` receives the variables
///     separately and bakes them into the image, which is also why a Nixpacks
///     app's secrets end up in its image layers.
///   * The auto-rollback path had lost three fields rather than mis-spelling one:
///     it sent no environment under either key, and no `memory_mb`/`cpu_percent`,
///     so a container that crashed and rolled back came back up stripped of its
///     configuration AND of its resource limits.
///
/// The key sent is `env`, the one the agent has always read. Sending the corrected
/// spelling instead would have fixed nothing until every agent in the fleet was
/// updated too, and agents update on their own schedule. Exactly one of the two
/// keys is sent: serde treats a field plus its alias as a duplicate field and
/// rejects the whole request.
/// Coerce the `env_vars` JSONB into the object of strings the agent deserializes.
///
/// The agent reads this field as `HashMap<String, String>`. `env_vars` is an
/// unconstrained JSONB column, and while `CreateRequest`/`UpdateRequest` both type
/// it as a string map, nothing stops a row written by hand — or by a future
/// writer, or a restored dump — from holding a number or a bool. Passing the raw
/// value through would make one such row fail the WHOLE deploy at the agent's
/// deserializer, which trades a bug that lost the environment for a bug that
/// refuses to deploy. Scalars are stringified, everything else is dropped.
fn env_object(env_vars: &serde_json::Value) -> serde_json::Value {
    let Some(obj) = env_vars.as_object() else {
        return serde_json::json!({});
    };
    let coerced: serde_json::Map<String, serde_json::Value> = obj
        .iter()
        .filter_map(|(k, v)| {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                // null / array / object have no sane env representation.
                _ => return None,
            };
            Some((k.clone(), serde_json::Value::String(s)))
        })
        .collect();
    serde_json::Value::Object(coerced)
}

pub(crate) fn build_deploy_body(b: DeployBody<'_>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": b.name,
        "image_tag": b.image_tag,
        "container_port": b.container_port,
        "host_port": b.host_port,
        "env": env_object(b.env_vars),
        "scope": b.scope,
    });
    // An emptied text column arrives here as Some("") rather than None, because
    // clearing a field is expressed as the empty string on the wire (the shape
    // v2.120.0 settled on for the alert destinations). Blank is absent for both
    // of these: a blank domain would have the agent write a vhost named
    // ".conf", and a blank ssl_email would have it open an ACME order with no
    // account address. Guard the value, not the Option.
    if let Some(d) = b.domain.filter(|d| !d.trim().is_empty()) {
        body["domain"] = serde_json::json!(d);
    }
    if let Some(mem) = b.memory_mb {
        body["memory_mb"] = serde_json::json!(mem);
    }
    if let Some(cpu) = b.cpu_percent {
        body["cpu_percent"] = serde_json::json!(cpu);
    }
    if let Some(email) = b.ssl_email.filter(|e| !e.trim().is_empty()) {
        body["ssl_email"] = serde_json::json!(email);
    }
    body
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct GitDeployHistory {
    pub id: Uuid,
    pub git_deploy_id: Uuid,
    pub commit_hash: String,
    pub commit_message: Option<String>,
    pub image_tag: String,
    pub status: String,
    pub output: Option<String>,
    pub triggered_by: String,
    pub duration_ms: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateRequest {
    pub name: String,
    pub repo_url: String,
    pub branch: Option<String>,
    pub dockerfile: Option<String>,
    pub container_port: Option<i32>,
    pub domain: Option<String>,
    pub env_vars: Option<HashMap<String, String>>,
    pub auto_deploy: Option<bool>,
    pub memory_mb: Option<i32>,
    pub cpu_percent: Option<i32>,
    pub ssl_email: Option<String>,
    pub pre_build_cmd: Option<String>,
    pub post_deploy_cmd: Option<String>,
    pub build_args: Option<HashMap<String, String>>,
    pub build_context: Option<String>,
    pub github_token: Option<String>,
    pub deploy_cron: Option<String>,
    pub deploy_protected: Option<bool>,
    pub preview_ttl_hours: Option<i32>,
}

#[derive(serde::Deserialize)]
pub struct UpdateRequest {
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    pub dockerfile: Option<String>,
    pub container_port: Option<i32>,
    pub domain: Option<String>,
    pub env_vars: Option<HashMap<String, String>>,
    pub auto_deploy: Option<bool>,
    pub memory_mb: Option<i32>,
    pub cpu_percent: Option<i32>,
    pub ssl_email: Option<String>,
    pub pre_build_cmd: Option<String>,
    pub post_deploy_cmd: Option<String>,
    pub build_args: Option<HashMap<String, String>>,
    pub build_context: Option<String>,
    pub github_token: Option<String>,
    pub deploy_cron: Option<String>,
    pub deploy_protected: Option<bool>,
    pub preview_ttl_hours: Option<i32>,
}

/// GET /api/git-deploys — List all git deploys for the current user.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, _agent): ServerScope,
) -> Result<Json<Vec<GitDeploy>>, ApiError> {
    require_admin(&claims.role)?;

    let mut deploys: Vec<GitDeploy> = sqlx::query_as(
        "SELECT * FROM git_deploys WHERE user_id = $1 AND server_id = $2 ORDER BY created_at DESC LIMIT 200",
    )
    .bind(claims.sub)
    .bind(server_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list git_deploys", e))?;

    // Mask github_token in responses
    for d in &mut deploys {
        mask_github_token(d);
    }

    Ok(Json(deploys))
}

/// POST /api/git-deploys — Create a new git deploy configuration.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    // The extractor stays even though the handle is unused: extracting it IS the
    // check that the caller owns the server they named, and `server_id` is what
    // the new row records as its host. Only the agent handle became redundant,
    // when the domain claim moved from asking this one host to asking the fleet.
    ServerScope(server_id, _agent): ServerScope,
    headers: axum::http::HeaderMap,
    Json(body): Json<CreateRequest>,
) -> Result<(StatusCode, Json<GitDeploy>), ApiError> {
    require_admin(&claims.role)?;

    if !is_valid_name(&body.name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid deploy name"));
    }

    if body.repo_url.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Repository URL is required"));
    }

    // Auto-allocate host_port: find first gap in 7000-7999 (scoped to this server)
    let used_ports: Vec<(i32,)> = sqlx::query_as(
        "SELECT host_port FROM git_deploys WHERE server_id = $1 ORDER BY host_port",
    )
    .bind(server_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("create git_deploys", e))?;

    let used: Vec<i32> = used_ports.into_iter().map(|(p,)| p).collect();
    let host_port = (7000..=7999)
        .find(|p| !used.contains(p))
        .ok_or_else(|| err(StatusCode::CONFLICT, "No available ports in range 7000-7999"))?;

    // Generate webhook secret
    let webhook_secret: String = {
        use rand::Rng;
        let bytes: Vec<u8> = (0..32).map(|_| rand::rng().random::<u8>()).collect();
        hex::encode(bytes)
    };

    let branch = body.branch.as_deref().unwrap_or("main");
    let dockerfile = body.dockerfile.as_deref().unwrap_or("Dockerfile");
    let container_port = body.container_port.unwrap_or(3000);
    let auto_deploy = body.auto_deploy.unwrap_or(false);
    let env_vars = body
        .env_vars
        .as_ref()
        .map(|e| serde_json::to_value(e).unwrap_or_default())
        .unwrap_or(serde_json::json!({}));
    let build_args = body
        .build_args
        .as_ref()
        .map(|e| serde_json::to_value(e).unwrap_or_default())
        .unwrap_or(serde_json::json!({}));
    let build_context = body.build_context.as_deref().unwrap_or(".");

    let deploy_protected = body.deploy_protected.unwrap_or(false);

    let preview_ttl = body.preview_ttl_hours.unwrap_or(24);

    // Validate pre_build_cmd and post_deploy_cmd for command injection
    if let Some(ref cmd) = body.pre_build_cmd {
        if !cmd.trim().is_empty() {
            super::is_safe_shell_command(cmd)
                .map_err(|e| err(StatusCode::BAD_REQUEST, &format!("pre_build_cmd: {e}")))?;
        }
    }
    if let Some(ref cmd) = body.post_deploy_cmd {
        if !cmd.trim().is_empty() {
            super::is_safe_shell_command(cmd)
                .map_err(|e| err(StatusCode::BAD_REQUEST, &format!("post_deploy_cmd: {e}")))?;
        }
    }

    // Validate build_context (prevent path traversal)
    if build_context.contains("..") || build_context.starts_with('/') {
        return Err(err(StatusCode::BAD_REQUEST, "build_context must not contain '..' or start with '/'"));
    }

    // Validate deploy_cron format (reject unparseable crons that would be stored and mis-scheduled)
    if let Some(ref cron) = body.deploy_cron {
        if !cron.trim().is_empty() && !is_valid_cron(cron) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid deploy_cron: expected at least 5 crontab fields"));
        }
    }

    // Format, reserved and every owner — see services::domain_claim. The two
    // conflict queries that used to be inlined here are the ones `update` never
    // grew, which is how a git deploy could be RENAMED onto an occupied domain
    // that it could not have been CREATED on.
    let domain = match body.domain.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => Some(
            domain_claim::ensure_claimable(
                &state.db,
                &state.agents,
                d,
                &headers,
                domain_claim::Holder::New,
                &claims.role,
            )
            .await?,
        ),
        // Was `body.domain.clone()`, which round-tripped a blank box back into the
        // column as ''. Harmless while the form sent null for an empty field;
        // load-bearing now that it sends the empty string, because `remove` reads
        // this column with `unwrap_or(&name)` and '' is not None.
        None => None,
    };

    // Encrypted at rest; `set_github_status` decrypts. `encrypt_stored_token`
    // also refuses the mask sentinel, so a client that echoes back the
    // ●●●●●●●● it was shown cannot overwrite a real token with it.
    let github_token_enc =
        encrypt_stored_token(body.github_token.as_deref(), &state.config.jwt_secret)?;

    let mut deploy: GitDeploy = sqlx::query_as(
        "INSERT INTO git_deploys (user_id, server_id, name, repo_url, branch, dockerfile, container_port, host_port, domain, env_vars, auto_deploy, webhook_secret, memory_mb, cpu_percent, ssl_email, pre_build_cmd, post_deploy_cmd, build_args, build_context, github_token, deploy_cron, deploy_protected, preview_ttl_hours) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23) \
         RETURNING *",
    )
    .bind(claims.sub)
    .bind(server_id)
    .bind(&body.name)
    .bind(body.repo_url.trim())
    .bind(branch)
    .bind(dockerfile)
    .bind(container_port)
    .bind(host_port)
    .bind(&domain)
    .bind(&env_vars)
    .bind(auto_deploy)
    .bind(&webhook_secret)
    .bind(body.memory_mb)
    .bind(body.cpu_percent)
    // The form posts the same object to create and to update, so these arrive
    // blank rather than absent whenever the operator left the box alone. NULL is
    // the stored spelling of "not set" on this table — every reader is written
    // against it — so the blank is normalised away here rather than stored and
    // guarded against at each of the readers for ever after.
    .bind(blank_to_none(body.ssl_email.as_deref()))
    .bind(blank_to_none(body.pre_build_cmd.as_deref()))
    .bind(blank_to_none(body.post_deploy_cmd.as_deref()))
    .bind(&build_args)
    .bind(build_context)
    .bind(&github_token_enc)
    .bind(blank_to_none(body.deploy_cron.as_deref()))
    .bind(deploy_protected)
    .bind(preview_ttl)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate key") {
            err(StatusCode::CONFLICT, "A deploy with this name already exists")
        } else {
            internal_error("create git_deploys", e)
        }
    })?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "git_deploy.create",
        Some("git_deploy"), Some(&body.name), None, None,
    ).await;

    // GAP 13: Auto-create webhook gateway endpoint for this git deploy
    {
        let gw_token = uuid::Uuid::new_v4().to_string().replace('-', "");
        let _ = sqlx::query(
            "INSERT INTO webhook_endpoints (user_id, name, description, token, verify_mode) \
             VALUES ($1, $2, $3, $4, 'none')"
        )
        .bind(claims.sub)
        .bind(format!("Git: {}", &body.name))
        .bind(format!("Auto-created for git deploy '{}'", &body.name))
        .bind(&gw_token)
        .execute(&state.db).await;
    }

    mask_github_token(&mut deploy);
    Ok((StatusCode::CREATED, Json(deploy)))
}

/// GET /api/git-deploys/{id} — Get a single git deploy.
pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<GitDeploy>, ApiError> {
    require_admin(&claims.role)?;

    let mut deploy: GitDeploy = sqlx::query_as(
        "SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("get_one git_deploys", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;

    mask_github_token(&mut deploy);
    Ok(Json(deploy))
}

/// PUT /api/git-deploys/{id} — Update a git deploy configuration.
pub async fn update(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(body): Json<UpdateRequest>,
) -> Result<Json<GitDeploy>, ApiError> {
    require_admin(&claims.role)?;

    // Verify ownership + fetch current domain/cron so we only validate values
    // that actually CHANGE (grandfather rows that predate these guards — e.g. a
    // deploy already on a reserved zone, or a stored 6-field cron — so an
    // unrelated field edit that re-sends the unchanged value isn't rejected).
    //
    // `server_id` is no longer fetched. It used to be, so the per-server conflict
    // check consulted the deploy's own host rather than the caller's header — a
    // real fix at the time. The claim is now fleet-wide on every leg (a domain
    // may exist once across the whole installation, and a Docker app holding it
    // is invisible to SQL on any host), so there is no per-server question left
    // for this handler to answer and nothing here would use the value.
    // The protection flag is fetched for one reason only: to tell a request that
    // changed it from one that merely re-sent it. The write below COALESCEs an
    // absent field onto the stored value, so "still true" and "set to true"
    // arrive here identically and only the previous value separates them.
    let existing: Option<(Option<String>, Option<String>, bool)> = sqlx::query_as(
        "SELECT domain, deploy_cron, deploy_protected FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("update git_deploys", e))?;

    let (cur_domain, cur_cron, was_protected) = match existing {
        Some(row) => row,
        None => return Err(err(StatusCode::NOT_FOUND, "Git deploy not found")),
    };

    // Validate domain ONLY when it changes. This used to run `is_valid_domain` +
    // `is_reserved_domain` and stop there, under a comment claiming parity with
    // `create` — but create ALSO ran two conflict queries, so a domain that could
    // not be created could still be renamed onto. The next deploy then rendered a
    // proxy vhost over the file the victim owned.
    let domain = match body.domain.as_deref() {
        Some(d) if !d.is_empty() && Some(d) != cur_domain.as_deref() => {
            // No agent is resolved here any more. The claim used to need one
            // because its Docker-app leg asked a single host; it now asks the
            // registry, which means a rename no longer 502s merely because the
            // target box is mid-reboot.
            Some(
                domain_claim::ensure_claimable(
                    &state.db,
                    &state.agents,
                    d,
                    &headers,
                    domain_claim::Holder::GitDeploy(id),
                    &claims.role,
                )
                .await?,
            )
        }
        // Emptying the box is now a real instruction rather than a no-op, so it
        // has to be answered rather than folded away. It is REFUSED, and the
        // refusal names the reason: nothing in the git path takes a vhost down.
        // `unexpose_domain` exists for Docker apps
        // (panel/agent/src/routes/docker_apps.rs), and `git_build.rs` says in
        // its own comment that wiring it here is separate work — until it is,
        // accepting the clear would drop the record of a config still proxying
        // to this container, which is the one state `services::ownership` was
        // written to keep out of the tree. Only fires when a domain is actually
        // stored: a deploy that never had one submits "" on every ordinary save.
        Some(d)
            if d.trim().is_empty()
                && cur_domain
                    .as_deref()
                    .map(|c| !c.trim().is_empty())
                    .unwrap_or(false) =>
        {
            return Err(err(
                StatusCode::BAD_REQUEST,
                &format!(
                    "Removing the domain is not supported yet: the nginx vhost for {} would keep \
                     proxying to this container and the panel has no way to take it down. Enter a \
                     different domain to move the deploy, or delete the deploy to remove the vhost.",
                    cur_domain.as_deref().unwrap_or("")
                ),
            ));
        }
        other => other.map(|d| d.to_string()),
    };

    // Validate deploy_cron format ONLY when it changes
    if let Some(ref cron) = body.deploy_cron {
        if !cron.trim().is_empty() && Some(cron) != cur_cron.as_ref() && !is_valid_cron(cron) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid deploy_cron: expected at least 5 crontab fields"));
        }
    }

    // Validate commands for injection (same as create)
    if let Some(ref cmd) = body.pre_build_cmd {
        if !cmd.trim().is_empty() {
            super::is_safe_shell_command(cmd)
                .map_err(|e| err(StatusCode::BAD_REQUEST, &format!("pre_build_cmd: {e}")))?;
        }
    }
    if let Some(ref cmd) = body.post_deploy_cmd {
        if !cmd.trim().is_empty() {
            super::is_safe_shell_command(cmd)
                .map_err(|e| err(StatusCode::BAD_REQUEST, &format!("post_deploy_cmd: {e}")))?;
        }
    }

    let env_vars = body.env_vars.as_ref().map(|e| serde_json::to_value(e).unwrap_or_default());
    let build_args = body.build_args.as_ref().map(|e| serde_json::to_value(e).unwrap_or_default());

    let github_token_enc =
        encrypt_stored_token(body.github_token.as_deref(), &state.config.jwt_secret)?;

    // THREE states on the wire, two of which used to be one. A COALESCE self-guard
    // folds an absent field onto the stored value, which is right for a key the
    // client omitted and wrong for a box the operator emptied — both arrive as
    // NULL, so "leave it alone" and "clear it" were indistinguishable and the
    // second silently became the first while the save answered 200.
    //
    //   key absent      -> NULL -> COALESCE keeps the stored value
    //   key sent as ""  -> ''   -> CASE writes NULL
    //   key sent with v -> v    -> CASE writes v
    //
    // The empty string is a WIRE sentinel and never a stored value. v2.120.0 fixed
    // the same defect on the alert destinations by letting '' reach the column, and
    // that is safe THERE because every reader of those columns guards on non-empty.
    // It is not safe here: `remove` falls back with `domain.as_deref().unwrap_or(&name)`,
    // so a stored '' would hand the agent an empty site identifier where a NULL
    // correctly yields the deploy's name. Normalising at the writer keeps every
    // existing Option-shaped reader out of the blast radius.
    //
    // `domain` is deliberately NOT in this list — see the refusal above; and
    // `github_token` is not either, because the GET masks it and the box is blank
    // on every ordinary edit.
    let mut deploy: GitDeploy = sqlx::query_as(
        "UPDATE git_deploys SET \
         repo_url = COALESCE($1, repo_url), \
         branch = COALESCE($2, branch), \
         dockerfile = COALESCE($3, dockerfile), \
         container_port = COALESCE($4, container_port), \
         domain = COALESCE($5, domain), \
         env_vars = COALESCE($6, env_vars), \
         auto_deploy = COALESCE($7, auto_deploy), \
         memory_mb = $8, \
         cpu_percent = $9, \
         ssl_email = CASE WHEN $10 = '' THEN NULL ELSE COALESCE($10, ssl_email) END, \
         pre_build_cmd = CASE WHEN $11 = '' THEN NULL ELSE COALESCE($11, pre_build_cmd) END, \
         post_deploy_cmd = CASE WHEN $12 = '' THEN NULL ELSE COALESCE($12, post_deploy_cmd) END, \
         build_args = COALESCE($13, build_args), \
         build_context = COALESCE($14, build_context), \
         github_token = COALESCE($15, github_token), \
         deploy_cron = CASE WHEN $16 = '' THEN NULL ELSE COALESCE($16, deploy_cron) END, \
         deploy_protected = COALESCE($17, deploy_protected), \
         preview_ttl_hours = COALESCE($18, preview_ttl_hours), \
         updated_at = NOW() \
         WHERE id = $19 AND user_id = $20 \
         RETURNING *",
    )
    .bind(body.repo_url.as_deref())
    .bind(body.branch.as_deref())
    .bind(body.dockerfile.as_deref())
    .bind(body.container_port)
    .bind(domain.as_deref())
    .bind(env_vars)
    .bind(body.auto_deploy)
    .bind(body.memory_mb)
    .bind(body.cpu_percent)
    .bind(body.ssl_email.as_deref())
    .bind(body.pre_build_cmd.as_deref())
    .bind(body.post_deploy_cmd.as_deref())
    .bind(build_args)
    .bind(body.build_context.as_deref())
    .bind(github_token_enc.as_deref())
    .bind(body.deploy_cron.as_deref())
    .bind(body.deploy_protected)
    .bind(body.preview_ttl_hours)
    .bind(id)
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("update git_deploys", e))?;

    // Turning the review requirement off is the one edit on this handler that
    // removes a control, and until now it was the only one that left no trace:
    // the row simply changed. It goes to the immutable log rather than the
    // activity feed because that table refuses UPDATE and DELETE, which is the
    // property an account disarming its own guard would otherwise exploit.
    //
    // Both directions are recorded. Only knowing when protection was switched
    // off tells you it was off; it does not tell you when it came back.
    if deploy.deploy_protected != was_protected {
        let (verb, severity) = if deploy.deploy_protected {
            ("enabled", "info")
        } else {
            ("disabled", "warning")
        };
        crate::services::security_hardening::audit_log(
            &state.db,
            "git_deploy.protection_changed",
            Some(&claims.email),
            crate::routes::client_ip(&headers).as_deref(),
            Some("git_deploy"),
            Some(&deploy.name),
            Some(&format!("Deploy approval requirement {verb}")),
            None,
            severity,
        )
        .await;
    }

    // Switching protection OFF must resolve anything still waiting on it.
    //
    // Nothing used to read these rows, so a request left behind by a flag that
    // had since been cleared was inert. It is not inert now: the panel lists
    // pending requests and offers Approve, and `approve_deploy` takes its target
    // from the row rather than re-deciding whether the deployment is protected.
    // A stale row would therefore sit in front of a second administrator, under a
    // deployment the same screen calls unprotected, one click from a production
    // deploy of whatever HEAD had become in the meantime.
    //
    // Resolved rather than deleted, for the same reason the migration keeps
    // resolved rows: they are the record of who decided what. 'cancelled' is a
    // third terminal state, distinct from a colleague's 'rejected'.
    if !deploy.deploy_protected {
        if let Err(e) = sqlx::query(
            "UPDATE deploy_approvals SET status = 'cancelled', resolved_at = NOW() \
             WHERE deploy_id = $1 AND status = 'pending'"
        )
        .bind(id)
        .execute(&state.db).await
        {
            tracing::warn!("Failed to cancel pending approvals for {id}: {e}");
        }
    }

    mask_github_token(&mut deploy);
    Ok(Json(deploy))
}

/// DELETE /api/git-deploys/{id} — Remove a git deploy and its container.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let deploy: Option<(String, Option<String>, Uuid)> = sqlx::query_as(
        "SELECT name, domain, server_id FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("remove git_deploys", e))?;

    let (name, domain, server_id) =
        deploy.ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;

    // WHICH deployment came from the row, so WHICH HOST has to come from the row
    // as well. It used to come from `ServerScope`: the caller's `X-Server-Id`
    // header, falling back to the local agent when there is no header at all. The
    // two agree on a one-box install and whenever the operator happens to have the
    // right server selected in the switcher, and disagree silently the rest of the
    // time.
    //
    // `trigger_deploy_task` already documents what that costs from the scheduler
    // side: `idx_git_deploys_name_server` makes `name` unique only PER SERVER while
    // the agent's checkout path is `/var/lib/dockpanel/git/{name}`, keyed by name
    // alone — so addressing the wrong box does not error, it finds the same-named
    // neighbour and acts on THAT. Removal is the sharpest form of it. `/git/cleanup`
    // stops the container and deletes its image, vhost, certificate and checkout,
    // and the calls below used to be fire-and-forget, so a teardown aimed at the
    // wrong machine destroyed another tenant's deployment and reported success.
    // They are checked now, and this handler refuses rather than deleting records
    // for containers the server did not confirm it removed.
    //
    // `server_id` is NOT NULL on this table, hence the bare `Some(...)`; the only
    // arm of the helper that can fire here is the unreachable-host one, and it
    // REFUSES rather than falling back to this box. Refusing is the correct answer:
    // the fallback was the defect.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(server_id),
        domain.as_deref().unwrap_or(&name),
    )
    .await?;

    // Every preview of this deployment is a container, a bound port, a vhost and
    // a checkout of its own, and the `git_previews` rows are about to be removed
    // by the FK cascade below — after which nothing on the box knows they exist.
    // Tear them down while the records that name them are still there.
    let previews: Vec<(String, Option<String>, i32)> = sqlx::query_as(
        "SELECT container_name, domain, host_port FROM git_previews WHERE git_deploy_id = $1",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let mut standing: Vec<String> = Vec::new();
    for (container_name, domain, host_port) in &previews {
        if let Err(e) = agent
            .post(
                "/git/cleanup",
                Some(preview_cleanup_body(container_name, domain.as_deref(), Some(*host_port))),
            )
            .await
        {
            tracing::warn!(
                "Failed to tear down preview {container_name} of git deploy {name}: {e}"
            );
            standing.push(container_name.clone());
        }
    }
    if !previews.is_empty() && standing.is_empty() {
        tracing::info!(
            "Removed {} preview container(s) alongside git deploy {name}",
            previews.len()
        );
    }

    // Tell agent to stop and remove container + cleanup. Discarding THIS result
    // was the same defect as the loop above — three lines apart, on the larger
    // subject: the deployment's own container, image, vhost, certificate and
    // checkout. The comment at the top of this function already said the calls
    // here were fire-and-forget and reported success; it described a defect
    // rather than a decision.
    if let Err(e) = agent
        .post(
            "/git/cleanup",
            Some(serde_json::json!({ "name": name, "scope": "deploy" })),
        )
        .await
    {
        tracing::warn!("Failed to tear down git deploy {name}: {e}");
        standing.push(name.clone());
    }

    // REFUSE, and name what is still up. The cascade below is the last moment
    // anything knows these containers exist: `git_previews` rows go with the
    // parent, so deleting anyway leaves containers running, holding their ports
    // and serving their vhosts, with nothing in the panel able to find them
    // again. Retryable by construction — `/git/cleanup` answers Ok for a
    // container that is already gone, so pressing Delete again once the server
    // answers finishes the job rather than double-deleting.
    //
    // This cannot become a permanent block: `agent_for_site_server` above
    // already refuses when the host is unreachable, so a decommissioned server
    // fails earlier than here and for a different reason.
    if !standing.is_empty() {
        return Err(err(
            StatusCode::CONFLICT,
            &format!(
                "The server did not confirm teardown of {}. Nothing was deleted — these \
                 records are the only thing that can still find those containers, their \
                 ports and their vhosts. Retry once the server answers.",
                standing.join(", ")
            ),
        ));
    }

    // Delete from DB (CASCADE deletes history)
    sqlx::query("DELETE FROM git_deploys WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove git_deploys", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "git_deploy.remove",
        Some("git_deploy"), Some(&name), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/git-deploys/{id}/deploy — Trigger a build+deploy (async with SSE progress).
pub async fn deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    // Check for active critical/major incidents — block deploy during outage
    let active_incidents: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM managed_incidents \
         WHERE status NOT IN ('resolved', 'postmortem') \
         AND severity IN ('critical', 'major')"
    ).fetch_one(&state.db).await.unwrap_or((0,));

    if active_incidents.0 > 0 {
        return Err(err(StatusCode::CONFLICT,
            "Deploy blocked: active critical/major incident in progress. Resolve the incident first."));
    }

    let config: GitDeploy = sqlx::query_as(
        "SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("deploy", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;

    // Protected deploy: require approval from another admin
    if config.deploy_protected {
        // File one pending request per deployment, not one per click.
        //
        // The conflict clause below lands on the partial unique index added with
        // this change, so the collapse is atomic rather than a read-then-write
        // that two concurrent clicks could both win. `rows_affected` is therefore
        // the honest answer to "did I create a request, or was one already
        // waiting?" — and the requester is told which, because the reason this
        // mattered is that they could not tell (see the migration).
        //
        // The clause is deliberately not spelled in this comment: a pin arm greps
        // raw source, and a quotation here would satisfy it while the code changed.
        let filed = sqlx::query(
            "INSERT INTO deploy_approvals (deploy_id, requested_by) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING"
        )
        .bind(id).bind(claims.sub)
        .execute(&state.db).await
        .map_err(|e| internal_error("deploy", e))?
        .rows_affected() > 0;

        // Only a NEW request is worth waking every administrator for. Re-announcing
        // one that is already waiting teaches the approvers to ignore the bell,
        // which costs more than the missing notification would have.
        if filed {
            notifications::notify_panel(
                &state.db, None,
                "Deploy approval needed",
                &format!("Deploy to {} requires approval", config.name),
                "warning", "deploy", Some("/git-deploys"),
            ).await;
        }

        return Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
            "status": "pending_approval",
            "message": if filed {
                "Deploy requires approval from another admin \u{2014} the request is now waiting in Pending Approvals."
            } else {
                "This deployment already has a request waiting in Pending Approvals; another admin has not resolved it yet."
            },
        }))));
    }

    // Resolve the agent for the server this deployment LIVES ON — see `remove()`
    // for why the caller's `ServerScope` is the wrong authority. Deployed to the
    // wrong host this is not a stray teardown but a stray BUILD: the neighbour's
    // checkout of the same name is overwritten with this repository and restarted
    // under its own port and vhost.
    //
    // Resolved HERE, deliberately, and not earlier: an unreachable host must not
    // stop a protected deploy from being QUEUED for approval (the branch above
    // talks to no agent at all), and it must not happen after the lock below,
    // which would strand `status = 'building'` for the 30-minute self-heal window
    // on a deploy that never started. `trigger_deploy_task` orders itself the same
    // way and says so.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(config.server_id),
        config.domain.as_deref().unwrap_or(&config.name),
    )
    .await?;

    // Deploy lock (atomic): flip status to 'building' iff not already building.
    // The old guard queried git_deploy_history for a 'building'/'deploying'
    // status that is NEVER written there (all history rows are 'success'/'failed'),
    // so it always counted 0 and never fired. This conditional UPDATE is the real
    // lock — a losing concurrent caller gets 0 rows affected — and self-heals a
    // crashed deploy after 30 min (stale 'building'; longer than the worst-case
    // clone+build+deploy so a still-running build never releases its own lock).
    // A DB error is surfaced as 500, NOT mistaken for "already in progress".
    match sqlx::query(
        "UPDATE git_deploys SET status = 'building', updated_at = NOW() \
         WHERE id = $1 AND (status IS DISTINCT FROM 'building' OR updated_at < NOW() - INTERVAL '30 minutes')"
    ).bind(id).execute(&state.db).await {
        Ok(r) if r.rows_affected() == 0 => return Err(err(StatusCode::CONFLICT, "Deploy already in progress")),
        Ok(_) => {}
        Err(e) => return Err(internal_error("deploy lock", e)),
    }

    let deploy_id = Uuid::new_v4();

    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        deploy_id,
        claims.sub,
        32,
    );

    spawn_deploy_task(
        state,
        agent,
        deploy_id,
        config,
        claims.sub,
        claims.email,
        "manual",
    );

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "deploy_id": deploy_id,
        "message": "Deployment started",
    }))))
}

/// GET /api/git-deploys/deploy/{deploy_id}/log — SSE stream of deploy progress.
pub async fn deploy_log(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(deploy_id): Path<Uuid>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, axum::BoxError>>>, ApiError> {
    // This check used to skip itself when the id was absent from the owner map,
    // on the stated grounds that the lookup below would 404 anyway. It would
    // not: absent from *owners* and absent from *logs* are different questions,
    // and most writers never registered an owner — so the fall-through streamed
    // rollback, webhook, backup, migration and site provisioning logs to any
    // signed-in account. Absence is now a refusal, not a pass.
    let (snapshot, rx) = crate::helpers::open_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        deploy_id,
        claims.sub,
        "No active deploy",
    )?;

    let snapshot_stream = futures::stream::iter(
        snapshot.into_iter().map(|step| {
            let data = serde_json::to_string(&step).unwrap_or_default();
            Ok(Event::default().data(data))
        }),
    );

    let live_stream = BroadcastStream::new(rx).filter_map(|result| async {
        match result {
            Ok(step) => {
                let data = serde_json::to_string(&step).ok()?;
                Some(Ok(Event::default().data(data)))
            }
            Err(_) => None,
        }
    });

    Ok(
        Sse::new(snapshot_stream.chain(live_stream))
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping")),
    )
}

/// GET /api/git-deploys/{id}/history — List deploy history.
pub async fn history(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<GitDeployHistory>>, ApiError> {
    require_admin(&claims.role)?;

    // Verify ownership
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("history", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Git deploy not found"));
    }

    let entries: Vec<GitDeployHistory> = sqlx::query_as(
        "SELECT * FROM git_deploy_history WHERE git_deploy_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("history", e))?;

    Ok(Json(entries))
}

/// POST /api/git-deploys/{id}/rollback/{history_id} — Rollback to a previous image.
pub async fn rollback(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, history_id)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    let config: GitDeploy = sqlx::query_as(
        "SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("rollback", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;

    let hist: GitDeployHistory = sqlx::query_as(
        "SELECT * FROM git_deploy_history WHERE id = $1 AND git_deploy_id = $2",
    )
    .bind(history_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("rollback", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "History entry not found"))?;

    // The host comes from the row, not from the caller's server switcher — see
    // `remove()`. Rollback carries its own edge: `hist.image_tag` names an image
    // built on THIS deployment's server and present in that daemon's local store,
    // so pointed at another box `/git/deploy` either fails on a missing image or,
    // worse, finds a same-tagged image the neighbour built and starts it under the
    // neighbour's name. Resolved before the status write below so an unreachable
    // host cannot strand `status = 'building'`.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(config.server_id),
        config.domain.as_deref().unwrap_or(&config.name),
    )
    .await?;

    // The same lock the other three build doors take, and for the same reason —
    // this one wrote the status unconditionally and only warned on failure, so a
    // rollback could start on top of a running build and each would swap the
    // container out from under the other. It is the last door that acquired it.
    match sqlx::query(
        "UPDATE git_deploys SET status = 'building', updated_at = NOW() \
         WHERE id = $1 AND (status IS DISTINCT FROM 'building' OR updated_at < NOW() - INTERVAL '30 minutes')"
    ).bind(id).execute(&state.db).await {
        Ok(r) if r.rows_affected() == 0 => return Err(err(StatusCode::CONFLICT, "A deploy or rollback is already in progress")),
        Ok(_) => {}
        Err(e) => return Err(internal_error("rollback lock", e)),
    }

    let deploy_id = Uuid::new_v4();

    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        deploy_id,
        claims.sub,
        32,
    );

    let logs = state.provision_logs.clone();
    let db = state.db.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();
    let deploy_name = config.name.clone();
    let rollback_image = hist.image_tag.clone();
    let rollback_commit = hist.commit_hash.clone();

    tokio::spawn(async move {
        let started = Instant::now();

        let emit = |step: &str, label: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(), label: label.into(), status: status.into(), message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&deploy_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        // Skip clone+build — go straight to deploy with the historical image
        emit("deploy", "Rolling back container", "in_progress", None);

        let deploy_body = build_deploy_body(DeployBody {
            name: &config.name,
            image_tag: &rollback_image,
            container_port: config.container_port,
            host_port: config.host_port,
            env_vars: &config.env_vars,
            domain: config.domain.as_deref(),
            memory_mb: config.memory_mb,
            cpu_percent: config.cpu_percent,
            ssl_email: config.ssl_email.as_deref(),
            scope: "deploy",
        });

        match agent.post_long("/git/deploy", Some(deploy_body), 120).await {
            Ok(result) => {
                let blue_green = result.get("blue_green").and_then(|v| v.as_bool()).unwrap_or(false);
                if blue_green {
                    emit("deploy", "Rolling back container", "done", Some("Zero-downtime swap".into()));
                } else {
                    emit("deploy", "Rolling back container", "done", None);
                }
                emit("complete", "Rollback complete", "done", None);

                let container_id = result.get("container_id").and_then(|v| v.as_str()).unwrap_or("");
                let duration_ms = started.elapsed().as_millis() as i32;

                // Record history
                if let Err(e) = sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, $2, $3, $4, 'success', $5, 'rollback', $6)",
                )
                .bind(id)
                .bind(&rollback_commit)
                .bind(format!("Rollback to {}", &rollback_commit[..7.min(rollback_commit.len())]))
                .bind(&rollback_image)
                .bind(format!("Rolled back to image {rollback_image}"))
                .bind(duration_ms)
                .execute(&db)
                .await
                {
                    tracing::warn!("Failed to record git deploy rollback history: {e}");
                }

                // Update git_deploys
                if let Err(e) = sqlx::query(
                    "UPDATE git_deploys SET status = 'running', container_id = $1, image_tag = $2, last_deploy = NOW(), last_commit = $3, updated_at = NOW() WHERE id = $4",
                )
                .bind(container_id)
                .bind(&rollback_image)
                .bind(&rollback_commit)
                .bind(id)
                .execute(&db)
                .await
                {
                    tracing::warn!("Failed to update git deploy status: {e}");
                }

                tracing::info!("Git deploy rollback success: {deploy_name} → {rollback_image}");
                activity::log_activity(
                    &db, user_id, &email, "git_deploy.rollback",
                    Some("git_deploy"), Some(&deploy_name), Some(&rollback_image), None,
                ).await;

                // Panel notification
                notifications::notify_panel(&db, Some(user_id), &format!("Rollback complete: {}", deploy_name), &format!("Rolled back to {}", &rollback_commit[..7.min(rollback_commit.len())]), "info", "deploy", Some("/git-deploys")).await;
            }
            Err(e) => {
                emit("deploy", "Rolling back container", "error", Some(format!("{e}")));
                emit("complete", "Rollback failed", "error", None);

                let duration_ms = started.elapsed().as_millis() as i32;

                if let Err(db_err) = sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, $2, $3, 'failed', $4, 'rollback', $5)",
                )
                .bind(id)
                .bind(&rollback_commit)
                .bind(&rollback_image)
                .bind(format!("{e}"))
                .bind(duration_ms)
                .execute(&db)
                .await
                {
                    tracing::warn!("Failed to record git deploy rollback history: {db_err}");
                }

                if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                    .bind(id)
                    .execute(&db)
                    .await
                {
                    tracing::warn!("Failed to update git deploy status: {db_err}");
                }

                tracing::error!("Git deploy rollback failed: {deploy_name}: {e}");

                // Panel notification
                notifications::notify_panel(&db, Some(user_id), &format!("Rollback failed: {}", deploy_name), &format!("{e}"), "critical", "deploy", Some("/git-deploys")).await;
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&deploy_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "deploy_id": deploy_id,
        "message": "Rollback started",
    }))))
}

/// POST /api/git-deploys/{id}/keygen — Generate SSH deploy key.
pub async fn keygen(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let deploy: Option<(String, Option<String>, Uuid)> = sqlx::query_as(
        "SELECT name, domain, server_id FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("keygen", e))?;

    let (name, domain, server_id) =
        deploy.ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;

    // The key has to be minted on the machine that will USE it — see `remove()`
    // for why the host comes from the row. A keypair generated on the panel host
    // is written to that box's `key_path`, and the path is then stored against a
    // deployment whose clones run somewhere else: the public half gets added to
    // the GitHub repo, the private half never reaches the server that needs it,
    // and every subsequent clone fails authentication for a key the panel says
    // exists. Worse on collision — it can overwrite the same-named neighbour's key.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(server_id),
        domain.as_deref().unwrap_or(&name),
    )
    .await?;

    let result = agent
        .post("/git/keygen", Some(serde_json::json!({ "name": name })))
        .await
        .map_err(|e| agent_error("Deploy key generation", e))?;

    let public_key = result.get("public_key").and_then(|v| v.as_str()).unwrap_or("");
    let key_path = result.get("key_path").and_then(|v| v.as_str()).unwrap_or("");

    sqlx::query(
        "UPDATE git_deploys SET deploy_key_public = $1, deploy_key_path = $2, updated_at = NOW() WHERE id = $3",
    )
    .bind(public_key)
    .bind(key_path)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("keygen", e))?;

    Ok(Json(serde_json::json!({
        "public_key": public_key,
    })))
}

/// POST /api/git-deploys/{id}/stop
pub async fn stop(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let config: GitDeploy = sqlx::query_as("SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| internal_error("stop", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;
    // Host from the row, not from the switcher — see `remove()`. The lifecycle
    // trio addresses the container purely by `config.name`, which is unique only
    // per server, so the caller's scope deciding the host means a stop aimed at a
    // fleet member takes down the panel host's same-named container instead — and
    // then writes 'stopped' onto the row of the one still running.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(config.server_id),
        config.domain.as_deref().unwrap_or(&config.name),
    )
    .await?;
    agent.post("/git/stop", Some(serde_json::json!({ "name": config.name }))).await
        .map_err(|e| agent_error("Stop container", e))?;
    if let Err(e) = sqlx::query("UPDATE git_deploys SET status = 'stopped', updated_at = NOW() WHERE id = $1")
        .bind(id).execute(&state.db).await
    {
        tracing::warn!("Failed to update git deploy status: {e}");
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/git-deploys/{id}/start
pub async fn start(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let config: GitDeploy = sqlx::query_as("SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| internal_error("start", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;
    // Host from the row — see `stop()`.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(config.server_id),
        config.domain.as_deref().unwrap_or(&config.name),
    )
    .await?;
    agent.post("/git/start", Some(serde_json::json!({ "name": config.name }))).await
        .map_err(|e| agent_error("Start container", e))?;
    if let Err(e) = sqlx::query("UPDATE git_deploys SET status = 'running', updated_at = NOW() WHERE id = $1")
        .bind(id).execute(&state.db).await
    {
        tracing::warn!("Failed to update git deploy status: {e}");
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/git-deploys/{id}/restart
pub async fn restart(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let config: GitDeploy = sqlx::query_as("SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| internal_error("restart", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;
    // Host from the row — see `stop()`.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(config.server_id),
        config.domain.as_deref().unwrap_or(&config.name),
    )
    .await?;
    agent.post("/git/restart", Some(serde_json::json!({ "name": config.name }))).await
        .map_err(|e| agent_error("Restart container", e))?;
    if let Err(e) = sqlx::query("UPDATE git_deploys SET status = 'running', updated_at = NOW() WHERE id = $1")
        .bind(id).execute(&state.db).await
    {
        tracing::warn!("Failed to update git deploy status: {e}");
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/git-deploys/{id}/logs
pub async fn container_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let config: GitDeploy = sqlx::query_as("SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| internal_error("container logs", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;
    // Host from the row — see `stop()`. Read-only, but it fails in the direction
    // nobody checks: asked of the wrong box it returns the same-named neighbour's
    // stdout as if it were this deployment's, so an operator debugging an incident
    // reads another tenant's logs and believes them.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(config.server_id),
        config.domain.as_deref().unwrap_or(&config.name),
    )
    .await?;
    let result = agent.post("/git/logs", Some(serde_json::json!({ "name": config.name }))).await
        .map_err(|e| agent_error("Container logs", e))?;
    Ok(Json(result))
}

/// POST /api/webhooks/git/{id}/{secret} — Webhook endpoint (no auth).
pub async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, secret)): Path<(Uuid, String)>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate Content-Type
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.is_empty() && !content_type.contains("application/json") {
        return Err(err(StatusCode::BAD_REQUEST, "Content-Type must be application/json"));
    }

    // Rate limit: max 10 attempts per deploy per hour
    {
        let mut attempts = state.webhook_attempts.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = attempts.entry(id).or_insert((0, now));
        if now.duration_since(entry.1).as_secs() >= 3600 {
            *entry = (0, now);
        }
        if entry.0 >= crate::routes::WEBHOOK_ATTEMPT_LIMIT {
            return Err(err(StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded. Try again later."));
        }
    }

    // Fetch the git deploy config
    let config: GitDeploy = sqlx::query_as(
        "SELECT * FROM git_deploys WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("webhook", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Invalid webhook"))?;

    // Constant-time secret comparison via SHA256 hash
    let provided_hash = {
        let mut h = Sha256::new();
        h.update(secret.as_bytes());
        h.finalize()
    };
    let stored_hash = {
        let mut h = Sha256::new();
        h.update(config.webhook_secret.as_bytes());
        h.finalize()
    };
    if provided_hash != stored_hash {
        // Record failed attempt
        {
            let mut attempts = state.webhook_attempts.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let entry = attempts.entry(id).or_insert((0, now));
            if now.duration_since(entry.1).as_secs() >= 3600 {
                *entry = (1, now);
            } else {
                entry.0 += 1;
            }
        }
        return Err(err(StatusCode::NOT_FOUND, "Invalid webhook"));
    }

    if !config.auto_deploy {
        return Err(err(StatusCode::BAD_REQUEST, "Auto-deploy is not enabled for this project"));
    }

    // Check for active critical/major incidents — skip webhook deploy during outage
    let active_incidents: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM managed_incidents \
         WHERE status NOT IN ('resolved', 'postmortem') \
         AND severity IN ('critical', 'major')"
    ).fetch_one(&state.db).await.unwrap_or((0,));

    if active_incidents.0 > 0 {
        tracing::warn!("Deploy blocked for {}: active incident in progress", config.name);
        return Ok(Json(serde_json::json!({ "ok": false, "message": "Deploy skipped: active incident" })));
    }

    // Parse body to check branch (GitHub/GitLab push payload)
    let payload = serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_default();
    let push_branch = payload.get("ref")
        .and_then(|r| r.as_str())
        .and_then(|r| r.strip_prefix("refs/heads/"))
        .unwrap_or("");

    // Resolve the agent for the server this deployment lives on.
    //
    // A webhook has no authenticated caller, so it has no `ServerScope` to read
    // an `X-Server-Id` header from — which is why this used to reach for the
    // local agent. The row itself is the authority, and the same handler file
    // already establishes that pattern for `update` (see the comment at the
    // `server_id` fetch there: reading it from the ROW "means the guard consults
    // the server the deploy actually lives on, which is the one whose nginx it
    // will overwrite"). A push to a deployment owned by a remote server must not
    // build, replace containers and rewrite vhosts on the panel host.
    let agent = match state.agents.for_server(config.server_id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(
                "Webhook deploy for {} refused: its server {} is unreachable ({e}) — \
                 refusing to deploy on a different host",
                config.name, config.server_id
            );
            return Err(err(StatusCode::BAD_GATEWAY, "Deploy target server is unreachable"));
        }
    };

    // Handle branch deletion (GitHub sends after=0000... on delete)
    let is_branch_delete = payload.get("deleted").and_then(|v| v.as_bool()).unwrap_or(false)
        || payload.get("after").and_then(|v| v.as_str()).map(|s| s.chars().all(|c| c == '0')).unwrap_or(false);

    if is_branch_delete {
        // Clean up preview for this deleted branch
        let deleted = match sqlx::query_as::<_, (uuid::Uuid, String, Option<String>, i32)>(
            "SELECT id, container_name, domain, host_port FROM git_previews WHERE git_deploy_id = $1 AND branch = $2"
        )
        .bind(config.id)
        .bind(&push_branch)
        .fetch_optional(&state.db)
        .await
        {
            Ok(row) => row,
            Err(e) => { tracing::warn!("Failed to query git preview for cleanup: {e}"); None }
        };

        let mut preview_retained = false;
        if let Some((preview_id, container_name, domain, host_port)) = deleted {
            // Reached with NO authentication — the webhook route has no auth
            // extractor, and a `deleted: true` payload names the branch. Which
            // makes it the sharpest of the four preview-teardown call sites, and
            // the one that most needs to address the preview's own space.
            match agent
                .post(
                    "/git/cleanup",
                    Some(preview_cleanup_body(&container_name, domain.as_deref(), Some(host_port))),
                )
                .await
            {
                Ok(_) => {
                    if let Err(e) = sqlx::query("DELETE FROM git_previews WHERE id = $1")
                        .bind(preview_id)
                        .execute(&state.db)
                        .await
                    {
                        tracing::warn!("Failed to delete preview record for {container_name}: {e}");
                    }
                    tracing::info!("Cleaned up preview for deleted branch: {push_branch}");
                }
                Err(e) => {
                    // KEEP THE ROW, for the reason `preview_cleanup` gives at its own
                    // teardown: the row is the only record that this container, this
                    // port and this vhost exist. Retiring it while the container is
                    // still up hands the port to the next preview push, which then
                    // cannot bind, and takes the container out of the sweep, the
                    // previews list and the operator's Delete button alike. Nothing
                    // on the box reaps a preview that has no row.
                    //
                    // This door is the one that most needs the row kept, because
                    // deleting a branch is nobody's retryable action — no operator is
                    // watching this response. Keeping it is what leaves a way back,
                    // and the ways back all read this row: Delete Preview always, and
                    // `preview_cleanup` once the preview is ELIGIBLE, which is not the
                    // same as its five-minute cadence — an hour for a `deploying` or
                    // `failed` one, `preview_ttl_hours` for a `running` one, and NEVER
                    // when that is 0, the documented opt-out from automatic cleanup.
                    // So on an opted-out install the operator's button is the only
                    // path, which is precisely why the record has to survive.
                    preview_retained = true;
                    tracing::warn!(
                        "Failed to tear down preview {container_name} for deleted branch \
                         {push_branch}: {e}. Keeping the git_previews row so the cleanup \
                         sweep retries rather than orphaning the container."
                    );
                }
            }
        }

        return Ok(Json(serde_json::json!({
            "ok": true,
            "action": "branch_deleted",
            "branch": push_branch,
            "preview_retained": preview_retained,
        })));
    }

    if !push_branch.is_empty() && push_branch != config.branch {
        // Preview deployment for non-configured branches. Report what actually
        // happened: three paths abandon the push before anything is spawned, and
        // answering "triggered" for those told the pusher work had started in the
        // one place they would look for it — GitHub's own delivery log. Still a
        // 200, deliberately: the delivery itself succeeded and a non-2xx would put
        // GitHub into retry over a decision that will not change on a retry.
        return match handle_preview_deploy(&state, &agent, &config, push_branch, &payload).await {
            Ok(()) => Ok(Json(serde_json::json!({
                "ok": true,
                "message": format!("Preview deploy triggered for branch '{push_branch}'"),
            }))),
            Err(reason) => Ok(Json(serde_json::json!({
                "ok": false,
                "message": format!(
                    "No preview deploy for branch '{push_branch}': {reason}"
                ),
            }))),
        };
    }

    // Deploy lock (atomic — see deploy(); the old git_deploy_history guard was inert)
    match sqlx::query(
        "UPDATE git_deploys SET status = 'building', updated_at = NOW() \
         WHERE id = $1 AND (status IS DISTINCT FROM 'building' OR updated_at < NOW() - INTERVAL '30 minutes')"
    ).bind(id).execute(&state.db).await {
        Ok(r) if r.rows_affected() == 0 => {
            return Ok(Json(serde_json::json!({ "ok": false, "message": "Deploy already in progress, skipping" })));
        }
        Ok(_) => {}
        Err(e) => return Err(internal_error("webhook deploy lock", e)),
    }

    let deploy_id = Uuid::new_v4();

    // No claims here — a webhook deploy has no signed-in actor. The log belongs
    // to whoever owns the git deploy it was fired against.
    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        deploy_id,
        config.user_id,
        32,
    );

    // Get user email for activity log
    let user_email: Option<(String,)> = match sqlx::query_as(
        "SELECT email FROM users WHERE id = $1",
    )
    .bind(config.user_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(row) => row,
        Err(e) => { tracing::warn!("Failed to fetch user email for webhook deploy: {e}"); None }
    };

    let email = user_email.map(|(e,)| e).unwrap_or_default();
    let owner_id = config.user_id;

    spawn_deploy_task(
        state,
        agent,
        deploy_id,
        config,
        owner_id,
        email,
        "webhook",
    );

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": "Deploy triggered",
    })))
}

/// Spawn the background clone → build → deploy task.
fn spawn_deploy_task(
    state: AppState,
    agent: AgentHandle,
    deploy_id: Uuid,
    config: GitDeploy,
    user_id: Uuid,
    email: String,
    triggered_by: &str,
) {
    let logs = state.provision_logs.clone();
    let db = state.db.clone();
    let deploy_name = config.name.clone();
    let git_deploy_id = config.id;
    let triggered = triggered_by.to_string();

    tokio::spawn(async move {
        let started = Instant::now();

        let emit = |step: &str, label: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(), label: label.into(), status: status.into(), message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&deploy_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        // Set GitHub pending status
        if let Some(ref gh_token) = config.github_token {
            if !gh_token.is_empty() {
                let token = gh_token.clone();
                let repo = config.repo_url.clone();
                let target = config
                    .domain
                    .as_deref()
                    .map(|d| deploy_url(d, config.ssl_email.as_deref()));
                tokio::spawn(async move {
                    set_github_status(&token, &repo, "HEAD", "pending", target).await;
                });
            }
        }

        // Pre-deploy backup: snapshot before deploying (best-effort, don't block deploy on failure)
        if let Some(ref domain) = config.domain {
            emit("backup", "Pre-deploy backup", "in_progress", None);
            let _ = agent.post(
                &format!("/backups/{}/create", domain),
                Some(serde_json::json!({"reason": "pre-deploy"})),
            ).await;
            emit("backup", "Pre-deploy backup", "done", None);
            tracing::info!("Pre-deploy backup requested for {domain}");
        }

        // Build clone body
        let mut clone_body = serde_json::json!({
            "name": config.name,
            "repo_url": config.repo_url,
            "branch": config.branch,
        });
        if let Some(ref key_path) = config.deploy_key_path {
            clone_body["key_path"] = serde_json::json!(key_path);
        }

        // Step 1: Clone
        emit("clone", "Cloning repository", "in_progress", None);
        let clone_result = agent.post_long("/git/clone", Some(clone_body), 300).await;
        let (commit_hash, commit_message) = match &clone_result {
            Ok(result) => {
                emit("clone", "Cloning repository", "done", None);
                let hash = result.get("commit_hash").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                let msg = result.get("commit_message").and_then(|v| v.as_str()).map(|s| s.to_string());
                (hash, msg)
            }
            Err(e) => {
                emit("clone", "Cloning repository", "error", Some(format!("{e}")));
                emit("complete", "Deploy failed", "error", None);

                let duration_ms = started.elapsed().as_millis() as i32;
                if let Err(db_err) = sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, 'unknown', '', 'failed', $2, $3, $4)",
                )
                .bind(git_deploy_id)
                .bind(format!("Clone failed: {e}"))
                .bind(&triggered)
                .bind(duration_ms)
                .execute(&db)
                .await
                {
                    tracing::warn!("Failed to record git deploy history: {db_err}");
                }

                if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                    .bind(git_deploy_id)
                    .execute(&db)
                    .await
                {
                    tracing::warn!("Failed to update git deploy status: {db_err}");
                }

                tracing::error!("Git deploy clone failed: {deploy_name}: {e}");
                tokio::time::sleep(Duration::from_secs(60)).await;
                logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&deploy_id);
                return;
            }
        };

        // Check for docker-compose.yml — if found, use compose deployment path
        let compose_result = agent.post("/git/compose-check", Some(serde_json::json!({
            "name": config.name, "build_context": config.build_context,
        }))).await.ok();

        let is_compose = compose_result.as_ref()
            .and_then(|r| r.get("found"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if is_compose {
            // Compose deployment path
            emit("compose", "Deploying with Docker Compose", "in_progress", None);
            let yaml = compose_result.as_ref()
                .and_then(|r| r.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if let Some(reason) =
                compose_deploy_refusal(&db, &agent, config.server_id, config.user_id, yaml).await
            {
                emit(
                    "compose",
                    "Docker Compose refused",
                    "error",
                    Some(reason.clone()),
                );
                tracing::warn!("Git deploy {deploy_name}: compose refused: {reason}");
                record_failed_history(
                    &db,
                    git_deploy_id,
                    &commit_hash,
                    commit_message.as_deref().unwrap_or(""),
                    &reason,
                    &triggered,
                )
                .await;
                if let Err(db_err) = sqlx::query(
                    "UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1",
                )
                .bind(git_deploy_id)
                .execute(&db)
                .await
                {
                    tracing::warn!("Failed to update git deploy status: {db_err}");
                }
                logs.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&deploy_id);
                return;
            }

            match agent.post_long("/apps/compose/deploy", Some(serde_json::json!({
                "yaml": yaml,
                "stack_id": config.id.to_string(),
            })), 660).await
                .map_err(|e| format!("{e}"))
                .and_then(compose_outcome)
            {
                Ok(()) => {
                    emit("compose", "Docker Compose deployed", "done", None);
                    emit("complete", "Deploy complete (Compose)", "done", None);

                    let duration_ms = started.elapsed().as_millis() as i32;
                    if let Err(db_err) = sqlx::query("INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) VALUES ($1, $2, $3, 'compose', 'success', 'Deployed via Docker Compose', $4, $5)")
                        .bind(git_deploy_id).bind(&commit_hash).bind(&commit_message).bind(&triggered).bind(duration_ms)
                        .execute(&db).await
                    {
                        tracing::warn!("Failed to record git deploy history: {db_err}");
                    }

                    if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'running', build_method = 'compose', last_deploy = NOW(), last_commit = $1, updated_at = NOW() WHERE id = $2")
                        .bind(&commit_hash).bind(git_deploy_id).execute(&db).await
                    {
                        tracing::warn!("Failed to update git deploy status: {db_err}");
                    }

                    tracing::info!("Git deploy (compose) success: {deploy_name} ({commit_hash})");
                    crate::services::activity::log_activity(&db, user_id, &email, "git_deploy.compose", Some("git_deploy"), Some(&deploy_name), Some(&commit_hash), Some("success")).await;
                }
                Err(e) => {
                    emit("compose", "Docker Compose deploy failed", "error", Some(format!("{e}")));
                    emit("complete", "Deploy failed", "error", None);
                    let duration_ms = started.elapsed().as_millis() as i32;
                    if let Err(db_err) = sqlx::query("INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) VALUES ($1, $2, $3, '', 'failed', $4, $5, $6)")
                        .bind(git_deploy_id).bind(&commit_hash).bind(&commit_message).bind(format!("Compose failed: {e}")).bind(&triggered).bind(duration_ms)
                        .execute(&db).await
                    {
                        tracing::warn!("Failed to record git deploy history: {db_err}");
                    }
                    if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                        .bind(git_deploy_id).execute(&db).await
                    {
                        tracing::warn!("Failed to update git deploy status: {db_err}");
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(60)).await;
            logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&deploy_id);
            return; // Skip single-container deployment path
        }

        // Try Nixpacks first, then fall back to auto-detect
        let mut nixpacks_image: Option<String> = None;
        emit("detect", "Detecting build method", "in_progress", None);
        match agent.post_long("/git/nixpacks-build", Some(serde_json::json!({
            "name": config.name,
            "commit_hash": commit_hash,
            "build_context": &config.build_context,
            "env_vars": config.env_vars,
        })), 660).await {
            Ok(result) => {
                nixpacks_image = result.get("image_tag").and_then(|v| v.as_str()).map(|s| s.to_string());
                emit("detect", "Built with Nixpacks", "done", None);
                if let Err(db_err) = sqlx::query("UPDATE git_deploys SET build_method = 'nixpacks', updated_at = NOW() WHERE id = $1")
                    .bind(git_deploy_id).execute(&db).await
                {
                    tracing::warn!("Failed to update git deploy build method: {db_err}");
                }
            }
            Err(_) => {
                // Nixpacks failed or not available — fall back to auto-detect
                match agent.post("/git/auto-detect", Some(serde_json::json!({
                    "name": config.name, "dockerfile": config.dockerfile, "build_context": config.build_context,
                }))).await {
                    Ok(result) => {
                        let auto = result.get("auto_generated").and_then(|v| v.as_bool()).unwrap_or(false);
                        if auto {
                            emit("detect", "Auto-detected project type", "done", None);
                            if let Err(db_err) = sqlx::query("UPDATE git_deploys SET build_method = 'auto-detect', updated_at = NOW() WHERE id = $1")
                                .bind(git_deploy_id).execute(&db).await
                            {
                                tracing::warn!("Failed to update git deploy build method: {db_err}");
                            }
                        } else {
                            emit("detect", "Using existing Dockerfile", "done", None);
                            if let Err(db_err) = sqlx::query("UPDATE git_deploys SET build_method = 'dockerfile', updated_at = NOW() WHERE id = $1")
                                .bind(git_deploy_id).execute(&db).await
                            {
                                tracing::warn!("Failed to update git deploy build method: {db_err}");
                            }
                        }
                    }
                    Err(e) => {
                        emit("detect", "No Dockerfile and auto-detect failed", "error", Some(format!("{e}")));
                        emit("complete", "Deploy failed", "error", None);
                        let duration_ms = started.elapsed().as_millis() as i32;
                        if let Err(db_err) = sqlx::query("INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) VALUES ($1, $2, $3, '', 'failed', $4, $5, $6)")
                            .bind(git_deploy_id).bind(&commit_hash).bind(&commit_message)
                            .bind(format!("Auto-detect failed: {e}")).bind(&triggered).bind(duration_ms)
                            .execute(&db).await
                        {
                            tracing::warn!("Failed to record git deploy history: {db_err}");
                        }
                        if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                            .bind(git_deploy_id).execute(&db).await
                        {
                            tracing::warn!("Failed to update git deploy status: {db_err}");
                        }
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&deploy_id);
                        return;
                    }
                }
            }
        }

        // Pre-build hook (runs in git dir on host, before docker build)
        if let Some(ref cmd) = config.pre_build_cmd {
            if !cmd.trim().is_empty() {
                emit("pre_build", "Running pre-build hook", "in_progress", None);
                match agent.post_long("/git/pre-build-hook", Some(serde_json::json!({ "name": config.name, "command": cmd })), 330).await {
                    Ok(result) => {
                        let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                        if success {
                            emit("pre_build", "Running pre-build hook", "done", None);
                        } else {
                            let output = result.get("output").and_then(|v| v.as_str()).unwrap_or("");
                            emit("pre_build", "Pre-build hook failed", "error", Some(output.to_string()));
                        }
                    }
                    Err(e) => {
                        emit("pre_build", "Pre-build hook failed", "error", Some(format!("{e}")));
                    }
                }
            }
        }

        // Step 2: Build (skip if nixpacks already built the image)
        let image_tag = if let Some(tag) = nixpacks_image {
            emit("build", "Image built by Nixpacks", "done", None);
            tag
        } else {
        emit("build", "Building Docker image", "in_progress", None);

        let build_body = serde_json::json!({
            "name": config.name,
            "dockerfile": config.dockerfile,
            "commit_hash": commit_hash,
            "build_args": config.build_args,
            "build_context": config.build_context,
        });

        match agent.post_long("/git/build", Some(build_body), 660).await {
            Ok(result) => {
                emit("build", "Building Docker image", "done", None);
                result.get("image_tag").and_then(|v| v.as_str()).unwrap_or("unknown").to_string()
            }
            Err(e) => {
                emit("build", "Building Docker image", "error", Some(format!("{e}")));
                emit("complete", "Deploy failed", "error", None);

                let duration_ms = started.elapsed().as_millis() as i32;
                if let Err(db_err) = sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, $2, $3, $4, 'failed', $5, $6, $7)",
                )
                .bind(git_deploy_id)
                .bind(&commit_hash)
                .bind(&commit_message)
                .bind("")
                .bind(format!("Build failed: {e}"))
                .bind(&triggered)
                .bind(duration_ms)
                .execute(&db)
                .await
                {
                    tracing::warn!("Failed to record git deploy history: {db_err}");
                }

                if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                    .bind(git_deploy_id)
                    .execute(&db)
                    .await
                {
                    tracing::warn!("Failed to update git deploy status: {db_err}");
                }

                tracing::error!("Git deploy build failed: {deploy_name}: {e}");
                tokio::time::sleep(Duration::from_secs(60)).await;
                logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&deploy_id);
                return;
            }
        }
        }; // end nixpacks_image if/else

        // Step 3: Deploy
        emit("deploy", "Deploying container", "in_progress", None);

        let deploy_body = build_deploy_body(DeployBody {
            name: &config.name,
            image_tag: &image_tag,
            container_port: config.container_port,
            host_port: config.host_port,
            env_vars: &config.env_vars,
            domain: config.domain.as_deref(),
            memory_mb: config.memory_mb,
            cpu_percent: config.cpu_percent,
            ssl_email: config.ssl_email.as_deref(),
            scope: "deploy",
        });

        match agent.post_long("/git/deploy", Some(deploy_body), 120).await {
            Ok(result) => {
                let blue_green = result.get("blue_green").and_then(|v| v.as_bool()).unwrap_or(false);
                if blue_green {
                    emit("deploy", "Deploying container", "done", Some("Zero-downtime swap".into()));
                } else {
                    emit("deploy", "Deploying container", "done", None);
                }
                emit("complete", "Deploy complete", "done", None);

                let container_id = result.get("container_id").and_then(|v| v.as_str()).unwrap_or("");
                let duration_ms = started.elapsed().as_millis() as i32;

                // Record success history
                if let Err(db_err) = sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, $2, $3, $4, 'success', $5, $6, $7)",
                )
                .bind(git_deploy_id)
                .bind(&commit_hash)
                .bind(&commit_message)
                .bind(&image_tag)
                .bind(if blue_green { "Deployed with zero-downtime swap" } else { "Deployed successfully" })
                .bind(&triggered)
                .bind(duration_ms)
                .execute(&db)
                .await
                {
                    tracing::warn!("Failed to record git deploy history: {db_err}");
                }

                // Update git_deploys
                if let Err(db_err) = sqlx::query(
                    "UPDATE git_deploys SET status = 'running', container_id = $1, image_tag = $2, last_deploy = NOW(), last_commit = $3, updated_at = NOW() WHERE id = $4",
                )
                .bind(container_id)
                .bind(&image_tag)
                .bind(&commit_hash)
                .bind(git_deploy_id)
                .execute(&db)
                .await
                {
                    tracing::warn!("Failed to update git deploy status: {db_err}");
                }

                tracing::info!("Git deploy success: {deploy_name} ({commit_hash})");
                activity::log_activity(
                    &db, user_id, &email, "git_deploy.deploy",
                    Some("git_deploy"), Some(&deploy_name), Some(&commit_hash), Some("success"),
                ).await;

                // Panel notification
                notifications::notify_panel(&db, Some(user_id), &format!("Deploy complete: {}", deploy_name), &format!("Commit: {}", commit_hash), "info", "deploy", Some("/git-deploys")).await;

                // Post-deploy hook
                if let Some(ref cmd) = config.post_deploy_cmd {
                    if !cmd.trim().is_empty() {
                        emit("post_deploy", "Running post-deploy hook", "in_progress", None);
                        match agent.post_long("/git/hook", Some(serde_json::json!({ "name": config.name, "command": cmd })), 330).await {
                            Ok(result) => {
                                let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                                let output = result.get("output").and_then(|v| v.as_str()).unwrap_or("");
                                if success {
                                    emit("post_deploy", "Post-deploy hook complete", "done", None);
                                } else {
                                    emit("post_deploy", "Post-deploy hook failed", "error", Some(output.to_string()));
                                }
                            }
                            Err(e) => {
                                emit("post_deploy", "Post-deploy hook failed", "error", Some(format!("{e}")));
                            }
                        }
                    }
                }

                // Deploy notification
                {
                    let notify_db = db.clone();
                    let notify_name = deploy_name.clone();
                    let notify_commit = commit_hash.clone();
                    let notify_user = user_id;
                    tokio::spawn(async move {
                        if let Some(channels) = crate::services::notifications::get_user_channels(&notify_db, notify_user, None).await {
                            let subject = format!("Deploy successful: {notify_name}");
                            let message = format!("Git deploy '{notify_name}' deployed successfully (commit: {notify_commit})");
                            let html = format!(
                                "<div style=\"font-family:sans-serif\"><h2 style=\"color:#22c55e\">Deploy Successful</h2>\
                                 <p><strong>{notify_name}</strong> deployed successfully.</p>\
                                 <p>Commit: <code>{notify_commit}</code></p>\
                                 <p style=\"color:#6b7280;font-size:14px\">Time: {}</p></div>",
                                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
                            );
                            crate::services::notifications::send_notification(&notify_db, &channels, &subject, &message, &html).await;
                        }
                    });
                }

                // GitHub commit status — success
                if let Some(ref gh_token) = config.github_token {
                    if !gh_token.is_empty() && commit_hash != "unknown" {
                        let token = gh_token.clone();
                        let repo_url = config.repo_url.clone();
                        let sha = commit_hash.clone();
                        let target = config
                            .domain
                            .as_deref()
                            .map(|d| deploy_url(d, config.ssl_email.as_deref()));
                        tokio::spawn(async move {
                            set_github_status(&token, &repo_url, &sha, "success", target).await;
                        });
                    }
                }

                // Post-deploy health check: verify site is responding
                if let Some(ref domain) = config.domain {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let check_url = deploy_url(domain, config.ssl_email.as_deref());
                    if let Ok(client) = reqwest::Client::builder()
                        .danger_accept_invalid_certs(true)
                        .timeout(std::time::Duration::from_secs(10))
                        .build()
                    {
                        match client.get(&check_url).send().await {
                            Ok(resp) => {
                                let status_code = resp.status().as_u16();
                                if status_code >= 500 {
                                    tracing::warn!("Post-deploy health check FAILED for {domain}: HTTP {status_code}");
                                    notifications::notify_panel(&db, Some(user_id),
                                        &format!("Deploy warning: {} returning HTTP {}", domain, status_code),
                                        &format!("Deploy succeeded but the site is returning HTTP {}. Check your application logs.", status_code),
                                        "warning", "deploy", Some("/git-deploys")).await;
                                } else {
                                    tracing::info!("Post-deploy health check OK for {domain}: HTTP {status_code}");
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Post-deploy health check FAILED for {domain}: {e}");
                                notifications::notify_panel(&db, Some(user_id),
                                    &format!("Deploy warning: {} unreachable", domain),
                                    &format!("Deploy succeeded but the site is not responding: {}", e),
                                    "warning", "deploy", Some("/git-deploys")).await;
                            }
                        }
                    }
                }

                // Auto-rollback monitor: watch container for 2 minutes after deploy
                {
                    let monitor_db = db.clone();
                    let monitor_agent = agent.clone();
                    let monitor_name = deploy_name.clone();
                    let monitor_gd_id = git_deploy_id;
                    let monitor_user = user_id;
                    let monitor_email_str = email.clone();
                    let monitor_image = image_tag.clone();
                    let monitor_config_name = config.name.clone();
                    let monitor_config_port = config.container_port;
                    let monitor_config_host_port = config.host_port;
                    let monitor_config_domain = config.domain.clone();
                    // Captured so the rollback below can redeploy the app as it was
                    // configured. Without these it rebuilt the container from the
                    // four fields above alone, which meant a crash-triggered
                    // rollback silently dropped the environment and the memory/CPU
                    // limits — the container came back both unconfigured and
                    // unbounded, on the one path nobody is watching (GH #94).
                    let monitor_config_env = config.env_vars.clone();
                    let monitor_config_memory = config.memory_mb;
                    let monitor_config_cpu = config.cpu_percent;
                    let monitor_config_ssl_email = config.ssl_email.clone();

                    tokio::spawn(async move {
                        // Check container health every 15s for 2 minutes
                        for _ in 0..8 {
                            tokio::time::sleep(Duration::from_secs(15)).await;

                            // Check if container is still running
                            match monitor_agent.post("/git/logs", Some(serde_json::json!({ "name": monitor_config_name, "lines": 1 }))).await {
                                Ok(_) => {} // Container is responding — alive
                                Err(_) => {
                                    // Container might be down — check status
                                    let container_name = format!("dockpanel-git-{monitor_config_name}");
                                    tracing::warn!("Auto-rollback: container {container_name} may have crashed, checking...");

                                    // Get last successful deploy before this one
                                    let prev: Option<(String, String)> = sqlx::query_as(
                                        "SELECT image_tag, commit_hash FROM git_deploy_history \
                                         WHERE git_deploy_id = $1 AND status = 'success' AND image_tag != $2 \
                                         ORDER BY created_at DESC LIMIT 1"
                                    )
                                    .bind(monitor_gd_id)
                                    .bind(&monitor_image)
                                    .fetch_optional(&monitor_db)
                                    .await
                                    .unwrap_or_else(|e| { tracing::warn!("Failed to fetch previous deploy for rollback: {e}"); None });

                                    if let Some((prev_image, prev_commit)) = prev {
                                        tracing::warn!("Auto-rollback: rolling back {monitor_name} to {prev_image}");

                                        // Deploy the previous image
                                        let rollback_body = build_deploy_body(DeployBody {
                                            name: &monitor_config_name,
                                            image_tag: &prev_image,
                                            container_port: monitor_config_port,
                                            host_port: monitor_config_host_port,
                                            env_vars: &monitor_config_env,
                                            domain: monitor_config_domain.as_deref(),
                                            memory_mb: monitor_config_memory,
                                            cpu_percent: monitor_config_cpu,
                                            ssl_email: monitor_config_ssl_email.as_deref(),
                                            scope: "deploy",
                                        });

                                        if monitor_agent.post_long("/git/deploy", Some(rollback_body), 120).await.is_ok() {
                                            // Record rollback in history
                                            if let Err(db_err) = sqlx::query(
                                                "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, image_tag, status, output, triggered_by) \
                                                 VALUES ($1, $2, $3, 'success', 'Auto-rollback after container crash', 'auto-rollback')"
                                            )
                                            .bind(monitor_gd_id)
                                            .bind(&prev_commit)
                                            .bind(&prev_image)
                                            .execute(&monitor_db)
                                            .await
                                            {
                                                tracing::warn!("Failed to record git deploy auto-rollback history: {db_err}");
                                            }

                                            // Update git_deploys
                                            if let Err(db_err) = sqlx::query("UPDATE git_deploys SET image_tag = $1, last_commit = $2, updated_at = NOW() WHERE id = $3")
                                                .bind(&prev_image)
                                                .bind(&prev_commit)
                                                .bind(monitor_gd_id)
                                                .execute(&monitor_db)
                                                .await
                                            {
                                                tracing::warn!("Failed to update git deploy status: {db_err}");
                                            }

                                            // Notify
                                            if let Some(channels) = crate::services::notifications::get_user_channels(&monitor_db, monitor_user, None).await {
                                                let subject = format!("Auto-rollback: {monitor_name}");
                                                let message = format!("Container '{monitor_name}' crashed after deploy. Auto-rolled back to {prev_commit}.");
                                                let html = format!(
                                                    "<div style=\"font-family:sans-serif\"><h2 style=\"color:#f59e0b\">Auto-Rollback</h2>\
                                                     <p>Container <strong>{monitor_name}</strong> crashed after deployment.</p>\
                                                     <p>Automatically rolled back to commit <code>{prev_commit}</code>.</p></div>"
                                                );
                                                crate::services::notifications::send_notification(&monitor_db, &channels, &subject, &message, &html).await;
                                            }

                                            activity::log_activity(
                                                &monitor_db, monitor_user, &monitor_email_str, "git_deploy.auto_rollback",
                                                Some("git_deploy"), Some(&monitor_name), Some(&prev_commit), None,
                                            ).await;

                                            // Panel notification
                                            notifications::notify_panel(&monitor_db, Some(monitor_user), &format!("Auto-rollback: {}", monitor_name), "Deploy failed, rolled back to previous version", "warning", "deploy", Some("/git-deploys")).await;

                                            tracing::info!("Auto-rollback complete: {monitor_name} → {prev_image}");
                                        }
                                    }
                                    return; // Stop monitoring after rollback
                                }
                            }
                        }
                        tracing::info!("Auto-rollback monitor: {monitor_name} healthy for 2 minutes, monitoring stopped");
                    });
                }
            }
            Err(e) => {
                emit("deploy", "Deploying container", "error", Some(format!("{e}")));
                emit("complete", "Deploy failed", "error", None);

                let duration_ms = started.elapsed().as_millis() as i32;

                if let Err(db_err) = sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, $2, $3, $4, 'failed', $5, $6, $7)",
                )
                .bind(git_deploy_id)
                .bind(&commit_hash)
                .bind(&commit_message)
                .bind(&image_tag)
                .bind(format!("Deploy failed: {e}"))
                .bind(&triggered)
                .bind(duration_ms)
                .execute(&db)
                .await
                {
                    tracing::warn!("Failed to record git deploy history: {db_err}");
                }

                if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                    .bind(git_deploy_id)
                    .execute(&db)
                    .await
                {
                    tracing::warn!("Failed to update git deploy status: {db_err}");
                }

                tracing::error!("Git deploy failed: {deploy_name}: {e}");
                activity::log_activity(
                    &db, user_id, &email, "git_deploy.deploy",
                    Some("git_deploy"), Some(&deploy_name), Some(&commit_hash), Some("failed"),
                ).await;

                // Panel notification
                notifications::notify_panel(&db, Some(user_id), &format!("Deploy failed: {}", deploy_name), &format!("{e}"), "critical", "deploy", Some("/git-deploys")).await;

                // GitHub commit status — failure
                if let Some(ref gh_token) = config.github_token {
                    if !gh_token.is_empty() && commit_hash != "unknown" {
                        let token = gh_token.clone();
                        let repo_url = config.repo_url.clone();
                        let sha = commit_hash.clone();
                        let target = config
                            .domain
                            .as_deref()
                            .map(|d| deploy_url(d, config.ssl_email.as_deref()));
                        tokio::spawn(async move {
                            set_github_status(&token, &repo_url, &sha, "failure", target).await;
                        });
                    }
                }

                // Deploy failure notification
                {
                    let notify_db = db.clone();
                    let notify_name = deploy_name.clone();
                    let notify_commit = commit_hash.clone();
                    let notify_user = user_id;
                    let notify_err = format!("{e}");
                    tokio::spawn(async move {
                        if let Some(channels) = crate::services::notifications::get_user_channels(&notify_db, notify_user, None).await {
                            let subject = format!("Deploy FAILED: {notify_name}");
                            let message = format!("Git deploy '{notify_name}' failed (commit: {notify_commit}): {notify_err}");
                            let html = format!(
                                "<div style=\"font-family:sans-serif\"><h2 style=\"color:#ef4444\">Deploy Failed</h2>\
                                 <p><strong>{notify_name}</strong> deployment failed.</p>\
                                 <p>Commit: <code>{notify_commit}</code></p>\
                                 <p>Error: {notify_err}</p>\
                                 <p style=\"color:#6b7280;font-size:14px\">Time: {}</p></div>",
                                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
                            );
                            crate::services::notifications::send_notification(&notify_db, &channels, &subject, &message, &html).await;
                        }
                    });
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&deploy_id);
    });
}

/// Mask github_token in API responses — show "●●●●●●●●" if set.
/// Everything a Compose document must satisfy before a git deploy hands it to
/// an agent, in one place because there are two callers and they had neither.
///
/// A repository's own `docker-compose.yml` reaches `/apps/compose/deploy` on
/// this path, so it is a deploy door like any other: it names registry images
/// an operator's CVE threshold applies to, and images their container policy
/// may restrict. It also skipped `validate_compose_yaml`, which the three
/// front-door compose paths run — not an escape, because the agent refuses any
/// bind outside `/var/lib/dockpanel/compose/` and blocks the Docker socket by
/// name, but it meant the same document was judged by different rules
/// depending on which door it arrived through.
///
/// Returns the sentence to report when the deploy must not proceed. Callers are
/// background tasks with no `?` to propagate through, which is why this hands
/// back a message rather than an `ApiError`.
/// Read the agent's per-service compose report and answer the only question the
/// caller actually asked: did this deploy happen?
///
/// `deploy_compose` reports each service's outcome INSIDE a 200 and never
/// returns an `Err` for a service that failed to start, so both callers here
/// used to take the HTTP status as the answer and write `status = 'running'`
/// over a deploy in which nothing came up. That is not a rare shape: the compose
/// engine has no teardown, so the SECOND compose deploy of the same deployment
/// collides with its own container names and every service fails — while the row
/// said the new commit was running. `stacks::deployed_service_states` is the
/// reader written for exactly this, and it is shared rather than copied so the
/// two paths cannot drift apart again.
///
/// ⚠ `total == 0` is deliberately NOT a failure. An agent that reports no
/// services at all is indistinguishable from one too old to report them, and an
/// empty observation is the one reading that cannot tell "nothing came up" from
/// "I cannot see". Only `total > 0 && running == 0` is a fact.
fn compose_outcome(deploy_result: serde_json::Value) -> Result<(), String> {
    let (running, total, errors) = crate::routes::stacks::deployed_service_states(&deploy_result);
    if total > 0 && running == 0 {
        let detail = if errors.is_empty() {
            "the agent reported no reason".to_string()
        } else {
            errors.join(" | ")
        };
        return Err(format!(
            "no service in the compose file stayed running — {detail}"
        ));
    }
    Ok(())
}

/// Refuse a compose file whose services name no image, rather than deploying
/// the rest of it and calling that a success.
///
/// A repository that has a Dockerfile is exactly the one whose compose file
/// builds from source, and the compose engine drops every service it cannot
/// resolve to a registry image. The deploy then comes up without the
/// application — the one service the author cared about — while the row says
/// `running`. Naming them is better than dropping them, and the alternative
/// offered is better than the thing refused: without a compose file this
/// deployment builds the Dockerfile and also gets the domain, the certificate,
/// zero-downtime swaps, preview environments and rollback.
///
/// ⚠ Kept OUT of `compose_deploy_refusal`'s body on purpose. The gate census in
/// `deploy-gate-coverage-pin-e2e.sh` attributes a `preflight_gate_image` call to
/// the nearest `fn` declared within the 40 lines above it, so prose added inside
/// that span pushes the declaration out of the window and the door reads as
/// ungated. Writing this arm inline cost exactly that — the census dropped from
/// 7 gated doors to 6 and named `trigger_deploy_task`, a door that does reach
/// the gate. Add explanation here, not there.
fn dropped_service_refusal(yaml: &str) -> Option<String> {
    let dropped = crate::routes::compose_services_without_image(yaml);
    if dropped.is_empty() {
        return None;
    }
    let (it, them) = if dropped.len() == 1 {
        ("it", "it")
    } else {
        ("they", "them")
    };
    Some(format!(
        "this repository's compose file gives no image for {}, and DockPanel's Compose support \
         runs registry images rather than building them — {it} would be skipped and the deploy \
         would come up without {them}. Either name an already-built image for each of them, or \
         remove the compose file: without it this deployment builds your Dockerfile and also \
         gets the domain, the certificate, zero-downtime swaps, preview environments and \
         rollback.",
        dropped.join(", "),
    ))
}

async fn compose_deploy_refusal(
    db: &sqlx::PgPool,
    agent: &crate::services::agent::AgentHandle,
    server_id: Uuid,
    user_id: Uuid,
    yaml: &str,
) -> Option<String> {
    fn sentence(e: crate::error::ApiError) -> String {
        e.1.0
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("refused")
            .to_string()
    }

    if let Err(e) = crate::routes::validate_compose_yaml(yaml) {
        return Some(e.to_string());
    }

    if let Some(reason) = dropped_service_refusal(yaml) {
        return Some(reason);
    }

    let images = crate::routes::compose_images(yaml);
    if let Err(e) = crate::routes::docker_apps::enforce_allowed_images(db, user_id, &images).await {
        return Some(sentence(e));
    }
    for image in &images {
        if let Err(e) =
            crate::routes::image_scans::preflight_gate_image(db, server_id, agent, image).await
        {
            return Some(sentence(e));
        }
    }
    None
}

/// The eight filled circles `mask_github_token` substitutes for a stored token.
/// Named once so the mask and the guard against re-storing it cannot drift apart.
const GITHUB_TOKEN_MASK: &str = "\u{25CF}\u{25CF}\u{25CF}\u{25CF}\u{25CF}\u{25CF}\u{25CF}\u{25CF}";

/// Encrypt a submitted GitHub token for storage.
///
/// `None` is passed through so `update`'s `COALESCE` keeps whatever is stored —
/// that is how the SPA leaves an unchanged token alone. The mask sentinel is
/// ALSO mapped to `None`: every handler now returns the masked row, so a client
/// that submits the form it was served sends the mask back, and storing it would
/// replace a working token with eight circles. Encrypting it would be worse
/// still — the value would look like a legitimate credential to every later
/// read. v2.48.3 shipped this class of bug once (an Edit button that re-encrypted
/// an already-encrypted destination password); it is closed here by construction
/// rather than by the caller remembering.
/// Replace any credentials embedded in a repository URL's authority with a
/// placeholder, leaving the rest of the URL readable.
///
/// The agent's `is_valid_repo_url` accepts `https://TOKEN@github.com/me/app.git`
/// and nothing strips it, so a token pasted into the Repository field is stored
/// verbatim. This does not un-store it — it stops the panel handing it to a
/// reader who is not its owner. Used on the deploy-approval list, whose reader
/// is an administrator of the machine rather than the deploy's operator.
pub fn mask_repo_credentials(url: &str) -> String {
    // Split on the authority, not on the first '@': a path may contain one.
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    match authority.rfind('@') {
        Some(at) => format!("{scheme}://•••{}{}", &authority[at..], tail),
        None => url.to_string(),
    }
}

/// The wire spells "not set" as an empty string; this table spells it NULL.
/// Translating at the writer is what keeps `Some("")` out of storage, and with it
/// out of every `if let Some(..)` and `unwrap_or` that reads these columns.
fn blank_to_none(v: Option<&str>) -> Option<&str> {
    v.filter(|s| !s.trim().is_empty())
}

fn encrypt_stored_token(
    submitted: Option<&str>,
    jwt_secret: &str,
) -> Result<Option<String>, ApiError> {
    match submitted {
        None => Ok(None),
        Some(t) if t.is_empty() || t == GITHUB_TOKEN_MASK => Ok(None),
        Some(t) => Ok(Some(
            crate::services::secrets_crypto::encrypt_credential(t, jwt_secret)
                .map_err(|e| internal_error("encrypt github token", e))?,
        )),
    }
}

fn mask_github_token(deploy: &mut GitDeploy) {
    if let Some(ref t) = deploy.github_token {
        if !t.is_empty() {
            deploy.github_token = Some(GITHUB_TOKEN_MASK.to_string());
        }
    }
}

/// The URL a git deploy's domain is actually reachable on.
///
/// A deploy only gets a certificate when an SSL email is configured — that is
/// the same condition `deploy_body` uses to ask the agent for one — so it is
/// also the condition that decides the scheme. This used to be assumed to be
/// https everywhere, under a comment reading "Git deploys with domain typically
/// have SSL"; on a deploy without one, the post-deploy health check then
/// connected to a port serving no TLS and reported a perfectly good deploy as
/// unreachable.
fn deploy_url(domain: &str, ssl_email: Option<&str>) -> String {
    let scheme = if ssl_email.is_some() { "https" } else { "http" };
    format!("{scheme}://{domain}")
}

/// Set GitHub commit status via the GitHub API.
///
/// Takes the finished URL rather than a domain, so every caller is forced
/// through `deploy_url` and none can reintroduce a hardcoded scheme in the link
/// third parties see on the commit.
async fn set_github_status(token: &str, repo_url: &str, sha: &str, state: &str, target_url: Option<String>) {
    // `git_deploys.github_token` is encrypted at rest by `create` and `update`.
    // Every one of the seven read sites in this module funnels here — either
    // directly or through a spawned task that clones the value first — and the
    // token never leaves the backend (the agent tree has zero references to it),
    // so this is the single place that has to open it, the same shape as
    // `helpers::cf_headers` and `cdn::bunny_headers`. The legacy fallback
    // returns a pre-encryption plaintext token unchanged, so existing rows keep
    // reporting commit status without a migration.
    let token = crate::services::secrets_crypto::decrypt_credential_from_env(token);
    let token = token.as_str();
    let (owner, repo) = match parse_github_repo(repo_url) {
        Some(r) => r,
        None => return, // Not a GitHub URL
    };

    let target_url = target_url.unwrap_or_default();
    let description = match state {
        "success" => "Deployed successfully via DockPanel",
        "failure" => "Deploy failed",
        "pending" => "Deploying...",
        _ => "Deploy status update",
    };

    let client = reqwest::Client::new();
    let _ = client
        .post(&format!("https://api.github.com/repos/{owner}/{repo}/statuses/{sha}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "DockPanel")
        .json(&serde_json::json!({
            "state": state,
            "target_url": target_url,
            "description": description,
            "context": "dockpanel/deploy",
        }))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
}

fn parse_github_repo(url: &str) -> Option<(String, String)> {
    // https://github.com/owner/repo.git
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        let clean = rest.trim_end_matches(".git");
        let parts: Vec<&str> = clean.splitn(2, '/').collect();
        if parts.len() == 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }
    // git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let clean = rest.trim_end_matches(".git");
        let parts: Vec<&str> = clean.splitn(2, '/').collect();
        if parts.len() == 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }
    None
}

/// Trigger a deploy task from the scheduler (no SSE, no provision logs).
pub async fn trigger_deploy_task(
    db: sqlx::PgPool,
    agents: crate::services::agent::AgentRegistry,
    git_deploy_id: Uuid,
    user_id: Uuid,
    triggered_by: String,
) {
    // Check for active critical/major incidents — skip scheduled deploy during outage
    let active_incidents: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM managed_incidents \
         WHERE status NOT IN ('resolved', 'postmortem') \
         AND severity IN ('critical', 'major')"
    ).fetch_one(&db).await.unwrap_or((0,));

    if active_incidents.0 > 0 {
        tracing::warn!("Scheduled deploy blocked for {git_deploy_id}: active incident in progress");
        return;
    }

    // Fetch config FIRST — before acquiring the lock — so a config-fetch error
    // (or a row deleted between the scheduler's list and now) returns WITHOUT
    // having flipped status to 'building' and stranding it for the self-heal window.
    let config: GitDeploy = match sqlx::query_as("SELECT * FROM git_deploys WHERE id = $1")
        .bind(git_deploy_id).fetch_optional(&db).await {
        Ok(Some(c)) => c,
        _ => return,
    };

    // Resolve the agent for the server this deployment LIVES ON, not for
    // whichever box happens to be running the panel.
    //
    // This used to be `AgentHandle::Local(agent)`, decided before the row was
    // even read. `git_deploys.server_id` is NOT NULL and
    // `idx_git_deploys_name_server` makes `name` unique only PER SERVER, so two
    // servers may legitimately own a deployment called the same thing — and the
    // agent's checkout path is `/var/lib/dockpanel/git/{name}`, keyed by name
    // alone. Driven on a two-box fleet against v2.56.0: a member's cron deploy
    // ran entirely on the panel host, and because both owned an `api`, whichever
    // cloned first owned the checkout while the other silently built the WRONG
    // repository into it and logged `Deploy success (scheduled)`. The member
    // never ran anything at all.
    //
    // Refuse rather than fall back to the local agent when the server cannot be
    // reached — the fallback is the defect, not the remedy.
    let agent = match agents.for_server(config.server_id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(
                "Scheduled deploy skipped for {} ({git_deploy_id}): its server {} is \
                 unreachable ({e}) — NOT deploying. Refusing to act on a different host.",
                config.name, config.server_id
            );
            return;
        }
    };

    let email: String = match sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(user_id).fetch_optional(&db).await {
        Ok(Some(e)) => e,
        Ok(None) => String::new(),
        Err(e) => {
            tracing::warn!("DB error fetching user email for git deploy: {e}");
            String::new()
        }
    };

    // Deploy lock (atomic — see deploy(); the old git_deploy_history guard was inert).
    // Acquired AFTER the config fetch above so a fetch error can't leave 'building' stuck.
    match sqlx::query(
        "UPDATE git_deploys SET status = 'building', updated_at = NOW() \
         WHERE id = $1 AND (status IS DISTINCT FROM 'building' OR updated_at < NOW() - INTERVAL '30 minutes')"
    ).bind(git_deploy_id).execute(&db).await {
        Ok(r) if r.rows_affected() == 0 => {
            tracing::warn!("Scheduled deploy skipped for {git_deploy_id}: deploy already in progress");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!("Scheduled deploy lock failed for {git_deploy_id}: {e}");
            return;
        }
    }

    let started = std::time::Instant::now();

    // GitHub pending status
    if let Some(ref gh_token) = config.github_token {
        if !gh_token.is_empty() {
            set_github_status(gh_token, &config.repo_url, "HEAD", "pending",
                config.domain.as_deref().map(|d| deploy_url(d, config.ssl_email.as_deref()))).await;
        }
    }

    // Pre-deploy backup: snapshot before deploying (best-effort, don't block deploy on failure)
    if let Some(ref domain) = config.domain {
        let _ = agent.post(
            &format!("/backups/{}/create", domain),
            Some(serde_json::json!({"reason": "pre-deploy"})),
        ).await;
        tracing::info!("Pre-deploy backup requested for {domain}");
    }

    // Clone
    let mut clone_body = serde_json::json!({
        "name": config.name, "repo_url": config.repo_url, "branch": config.branch,
    });
    if let Some(ref key_path) = config.deploy_key_path {
        clone_body["key_path"] = serde_json::json!(key_path);
    }

    let clone_result = agent.post_long("/git/clone", Some(clone_body), 300).await;

    let (commit_hash, commit_message) = match clone_result {
        Ok(r) => (
            r.get("commit_hash").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
            r.get("commit_message").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        ),
        Err(e) => {
            tracing::error!("Scheduled deploy clone failed: {}: {e}", config.name);
            record_failed_history(&db, git_deploy_id, "unknown", "", &format!("Clone failed: {e}"), &triggered_by).await;
            if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                .bind(git_deploy_id).execute(&db).await
            {
                tracing::warn!("Failed to update git deploy status: {db_err}");
            }
            return;
        }
    };

    // Check for docker-compose.yml — if found, use compose deployment path
    if let Ok(compose_result) = agent.post("/git/compose-check", Some(serde_json::json!({
        "name": config.name, "build_context": config.build_context,
    }))).await {
        let is_compose = compose_result.get("found").and_then(|v| v.as_bool()).unwrap_or(false);
        if is_compose {
            let yaml = compose_result.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(reason) =
                compose_deploy_refusal(&db, &agent, config.server_id, config.user_id, yaml).await
            {
                tracing::warn!("Git deploy {git_deploy_id}: compose refused: {reason}");
                record_failed_history(
                    &db,
                    git_deploy_id,
                    &commit_hash,
                    &commit_message,
                    &reason,
                    &triggered_by,
                )
                .await;
                if let Err(db_err) = sqlx::query(
                    "UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1",
                )
                .bind(git_deploy_id)
                .execute(&db)
                .await
                {
                    tracing::warn!("Failed to update git deploy status: {db_err}");
                }
                return;
            }
            match agent.post_long("/apps/compose/deploy", Some(serde_json::json!({
                "yaml": yaml, "stack_id": config.id.to_string(),
            })), 660).await
                .map_err(|e| format!("{e}"))
                .and_then(compose_outcome)
            {
                Ok(()) => {
                    let duration_ms = started.elapsed().as_millis() as i32;
                    if let Err(db_err) = sqlx::query("INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) VALUES ($1, $2, $3, 'compose', 'success', 'Deployed via Docker Compose', $4, $5)")
                        .bind(git_deploy_id).bind(&commit_hash).bind(&commit_message).bind(&triggered_by).bind(duration_ms)
                        .execute(&db).await
                    {
                        tracing::warn!("Failed to record git deploy history: {db_err}");
                    }
                    if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'running', build_method = 'compose', last_deploy = NOW(), last_commit = $1, updated_at = NOW() WHERE id = $2")
                        .bind(&commit_hash).bind(git_deploy_id).execute(&db).await
                    {
                        tracing::warn!("Failed to update git deploy status: {db_err}");
                    }
                    tracing::info!("Deploy success (compose/{}): {} ({commit_hash})", triggered_by, config.name);
                    crate::services::activity::log_activity(&db, user_id, &email, "git_deploy.compose", Some("git_deploy"), Some(&config.name), Some(&commit_hash), Some(&triggered_by)).await;
                }
                Err(e) => {
                    tracing::error!("Compose deploy failed ({}): {}: {e}", triggered_by, config.name);
                    record_failed_history(&db, git_deploy_id, &commit_hash, &commit_message, &format!("Compose failed: {e}"), &triggered_by).await;
                    if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                        .bind(git_deploy_id).execute(&db).await
                    {
                        tracing::warn!("Failed to update git deploy status: {db_err}");
                    }
                }
            }
            return; // Skip single-container path
        }
    }

    // Try Nixpacks first, then fall back to auto-detect + docker build
    let mut nixpacks_image: Option<String> = None;
    if let Ok(result) = agent.post_long("/git/nixpacks-build", Some(serde_json::json!({
        "name": config.name,
        "commit_hash": commit_hash,
        "build_context": &config.build_context,
        "env_vars": config.env_vars,
    })), 660).await {
        nixpacks_image = result.get("image_tag").and_then(|v| v.as_str()).map(|s| s.to_string());
        tracing::info!("Nixpacks build succeeded for {}", config.name);
        if let Err(db_err) = sqlx::query("UPDATE git_deploys SET build_method = 'nixpacks', updated_at = NOW() WHERE id = $1")
            .bind(git_deploy_id).execute(&db).await
        {
            tracing::warn!("Failed to update git deploy build method: {db_err}");
        }
    } else {
        // Nixpacks unavailable — try auto-detect
        if let Err(e) = agent.post("/git/auto-detect", Some(serde_json::json!({
            "name": config.name, "dockerfile": config.dockerfile, "build_context": config.build_context,
        }))).await {
            tracing::error!("Auto-detect failed ({}): {}: {e}", triggered_by, config.name);
            record_failed_history(&db, git_deploy_id, &commit_hash, &commit_message, &format!("Auto-detect failed: {e}"), &triggered_by).await;
            if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                .bind(git_deploy_id).execute(&db).await
            {
                tracing::warn!("Failed to update git deploy status: {db_err}");
            }
            return;
        }
        // Refresh the lock's self-heal clock before the (up to ~16 min) pre-build+build:
        // this fallthrough path has no updated_at bump since lock acquisition, so a
        // legitimately long Dockerfile build could otherwise cross the 30-min window and
        // let a concurrent trigger self-release the lock (mirrors spawn_deploy_task 1466/1473).
        if let Err(db_err) = sqlx::query("UPDATE git_deploys SET build_method = 'dockerfile', updated_at = NOW() WHERE id = $1")
            .bind(git_deploy_id).execute(&db).await
        {
            tracing::warn!("Failed to update git deploy build method: {db_err}");
        }
    }

    // Pre-build hook
    if let Some(ref cmd) = config.pre_build_cmd {
        if !cmd.trim().is_empty() {
            let _ = agent.post_long("/git/pre-build-hook", Some(serde_json::json!({
                "name": config.name, "command": cmd,
            })), 330).await;
        }
    }

    // Build (skip if nixpacks already built the image)
    let image_tag = if let Some(tag) = nixpacks_image {
        tag
    } else {
        match agent.post_long("/git/build", Some(serde_json::json!({
            "name": config.name, "dockerfile": config.dockerfile, "commit_hash": commit_hash,
            "build_args": config.build_args, "build_context": config.build_context,
        })), 660).await {
            Ok(r) => r.get("image_tag").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
            Err(e) => {
                tracing::error!("Scheduled deploy build failed: {}: {e}", config.name);
                record_failed_history(&db, git_deploy_id, &commit_hash, &commit_message, &format!("Build failed: {e}"), &triggered_by).await;
                if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                    .bind(git_deploy_id).execute(&db).await
                {
                    tracing::warn!("Failed to update git deploy status: {db_err}");
                }
                if let Some(ref gh_token) = config.github_token {
                    if !gh_token.is_empty() && commit_hash != "unknown" {
                        set_github_status(gh_token, &config.repo_url, &commit_hash, "failure",
                            config.domain.as_deref().map(|d| deploy_url(d, config.ssl_email.as_deref()))).await;
                    }
                }
                return;
            }
        }
    };

    // Deploy
    let deploy_body = build_deploy_body(DeployBody {
        name: &config.name,
        image_tag: &image_tag,
        container_port: config.container_port,
        host_port: config.host_port,
        env_vars: &config.env_vars,
        domain: config.domain.as_deref(),
        memory_mb: config.memory_mb,
        cpu_percent: config.cpu_percent,
        ssl_email: config.ssl_email.as_deref(),
        scope: "deploy",
    });

    match agent.post_long("/git/deploy", Some(deploy_body), 120).await {
        Ok(result) => {
            let container_id = result.get("container_id").and_then(|v| v.as_str()).unwrap_or("");
            let duration_ms = started.elapsed().as_millis() as i32;

            if let Err(db_err) = sqlx::query(
                "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, triggered_by, duration_ms) VALUES ($1, $2, $3, $4, 'success', $5, $6)"
            ).bind(git_deploy_id).bind(&commit_hash).bind(&commit_message).bind(&image_tag).bind(&triggered_by).bind(duration_ms)
            .execute(&db).await
            {
                tracing::warn!("Failed to record git deploy history: {db_err}");
            }

            if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'running', container_id = $1, image_tag = $2, last_deploy = NOW(), last_commit = $3, updated_at = NOW() WHERE id = $4")
                .bind(container_id).bind(&image_tag).bind(&commit_hash).bind(git_deploy_id).execute(&db).await
            {
                tracing::warn!("Failed to update git deploy status: {db_err}");
            }

            // Post-deploy hook
            if let Some(ref cmd) = config.post_deploy_cmd {
                if !cmd.trim().is_empty() {
                    let _ = agent.post_long("/git/hook", Some(serde_json::json!({ "name": config.name, "command": cmd })), 330).await;
                }
            }

            // GitHub status
            if let Some(ref gh_token) = config.github_token {
                if !gh_token.is_empty() && commit_hash != "unknown" {
                    set_github_status(gh_token, &config.repo_url, &commit_hash, "success",
                            config.domain.as_deref().map(|d| deploy_url(d, config.ssl_email.as_deref()))).await;
                }
            }

            // Notification
            if let Some(channels) = crate::services::notifications::get_user_channels(&db, user_id, None).await {
                let subject = format!("Deploy successful: {} ({})", config.name, triggered_by);
                let msg = format!("Git deploy '{}' deployed successfully (commit: {commit_hash})", config.name);
                crate::services::notifications::send_notification(&db, &channels, &subject, &msg, &msg).await;
            }

            tracing::info!("Deploy success ({}): {} ({commit_hash})", triggered_by, config.name);
            crate::services::activity::log_activity(&db, user_id, &email, "git_deploy.deploy", Some("git_deploy"), Some(&config.name), Some(&commit_hash), Some(&triggered_by)).await;

            // Post-deploy health check: verify site is responding
            if let Some(ref domain) = config.domain {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let check_url = deploy_url(domain, config.ssl_email.as_deref());
                if let Ok(client) = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                {
                    match client.get(&check_url).send().await {
                        Ok(resp) => {
                            let status_code = resp.status().as_u16();
                            if status_code >= 500 {
                                tracing::warn!("Post-deploy health check FAILED for {domain}: HTTP {status_code}");
                                notifications::notify_panel(&db, Some(user_id),
                                    &format!("Deploy warning: {} returning HTTP {}", domain, status_code),
                                    &format!("Deploy succeeded but the site is returning HTTP {}. Check your application logs.", status_code),
                                    "warning", "deploy", Some("/git-deploys")).await;
                            } else {
                                tracing::info!("Post-deploy health check OK for {domain}: HTTP {status_code}");
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Post-deploy health check FAILED for {domain}: {e}");
                            notifications::notify_panel(&db, Some(user_id),
                                &format!("Deploy warning: {} unreachable", domain),
                                &format!("Deploy succeeded but the site is not responding: {}", e),
                                "warning", "deploy", Some("/git-deploys")).await;
                        }
                    }
                }
            }
        }
        Err(e) => {
            let _duration_ms = started.elapsed().as_millis() as i32;
            record_failed_history(&db, git_deploy_id, &commit_hash, &commit_message, &format!("Deploy failed: {e}"), &triggered_by).await;
            if let Err(db_err) = sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1").bind(git_deploy_id).execute(&db).await {
                tracing::warn!("Failed to update git deploy status: {db_err}");
            }

            if let Some(ref gh_token) = config.github_token {
                if !gh_token.is_empty() && commit_hash != "unknown" {
                    set_github_status(gh_token, &config.repo_url, &commit_hash, "failure",
                            config.domain.as_deref().map(|d| deploy_url(d, config.ssl_email.as_deref()))).await;
                }
            }

            tracing::error!("Deploy failed ({}): {}: {e}", triggered_by, config.name);
        }
    }
}

async fn record_failed_history(db: &sqlx::PgPool, git_deploy_id: Uuid, commit_hash: &str, commit_message: &str, output: &str, triggered_by: &str) {
    if let Err(e) = sqlx::query(
        "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by) VALUES ($1, $2, $3, '', 'failed', $4, $5)"
    ).bind(git_deploy_id).bind(commit_hash).bind(commit_message).bind(output).bind(triggered_by).execute(db).await {
        tracing::warn!("Failed to record git deploy history: {e}");
    }
}

/// Handle preview deployment for non-configured branches.
///
/// Returns the reason the push was NOT taken up, when it was not. Four things
/// abandon a preview BEFORE the build task is spawned, and the webhook answered
/// `Preview deploy triggered` for all of them — a claim about work that never
/// started, delivered to the one place a pusher would look for it. Past the spawn
/// the answer is honest: a clone or build failure after that point is recorded on
/// the row as `status = 'failed'` and shown in the previews list.
///
/// The fourth was added last and is the reason that last sentence is true at all.
/// A failed `git_previews` upsert used to be logged and stepped over, and every
/// status write below is `WHERE git_deploy_id = $1 AND branch = $2` — so with no
/// row they all match zero rows and return `Ok`, and a preview that built, bound
/// a port, took a vhost and obtained a certificate was recorded nowhere and
/// reported as running. Nothing on the box reaps a preview that has no row.
async fn handle_preview_deploy(
    state: &AppState,
    agent: &AgentHandle,
    config: &GitDeploy,
    branch: &str,
    _payload: &serde_json::Value,
) -> Result<(), String> {
    let branch_slug = dns_label(branch);
    if branch_slug.len() > 50 {
        // Safety limit
        return Err(format!(
            "branch name '{branch}' is too long to form a preview host name"
        ));
    }

    // Allocate preview port (scoped to this server via git_deploys)
    let used_ports: Vec<(i32,)> = sqlx::query_as(
        "SELECT gp.host_port FROM git_previews gp \
         JOIN git_deploys gd ON gd.id = gp.git_deploy_id \
         WHERE gd.server_id = $1"
    )
    .bind(config.server_id)
    .fetch_all(&state.db).await.unwrap_or_default();
    let used: std::collections::HashSet<i32> = used_ports.into_iter().map(|(p,)| p).collect();
    let port = match (8000..=8999).find(|p| !used.contains(p)) {
        Some(p) => p,
        None => {
            tracing::warn!("No preview ports available");
            return Err("no preview port is free — 8000-8999 are all in use".to_string());
        }
    };

    // The preview's own name space. Before v2.55.0 this was
    // `dockpanel-git-{config}-pr-{slug}` — the container name of a DEPLOYMENT
    // called `{config}-pr-{slug}`, which any admin can create and which the
    // agent resolved by name alone. Both halves of that string are reachable by
    // whoever can push to the repo (the webhook has no auth extractor), so a
    // push could blue-green a stranger's production container out of existence
    // and repoint that stranger's domain at the pushed branch.
    let preview_name = format!("{}-pr-{}", config.name, branch_slug);
    let container_name = format!("dockpanel-git-{PREVIEW_SCOPE_PREFIX}{preview_name}");

    // A row written before the split names its container in the OLD shared
    // space. The upsert below overwrites that name, so unless it is torn down
    // first the container, its port, its vhost and its checkout are orphaned
    // with nothing left in the database pointing at them.
    let previous: Option<(String, Option<String>, i32)> = sqlx::query_as(
        "SELECT container_name, domain, host_port FROM git_previews \
         WHERE git_deploy_id = $1 AND branch = $2",
    )
    .bind(config.id)
    .bind(branch)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    if let Some((prev_name, prev_domain, prev_port)) = previous {
        if prev_name != container_name {
            tracing::info!(
                "Preview {prev_name} predates the preview name space; tearing it down before \
                 deploying {container_name}"
            );
            if let Err(e) = agent
                .post(
                    "/git/cleanup",
                    Some(preview_cleanup_body(
                        &prev_name,
                        prev_domain.as_deref(),
                        Some(prev_port),
                    )),
                )
                .await
            {
                // ABANDON THIS PUSH. The comment above the query says what the
                // upsert below costs if this teardown did not happen: it
                // overwrites `container_name`, and from that moment the
                // predecessor's container, port, vhost and checkout are orphaned
                // with nothing in the database naming them. Leaving the row
                // pointing at the container that still exists keeps it findable
                // by the sweep, by the previews list and by Delete Preview, and
                // the next push retries this teardown from the same state.
                tracing::warn!(
                    "Failed to tear down predecessor preview {prev_name}: {e}. Leaving the \
                     git_previews row pointing at it and skipping this preview deploy — \
                     overwriting the row here is what orphans the container."
                );
                return Err(format!(
                    "the previous preview container {prev_name} could not be torn down, so \
                     this push was not taken up — retry once the server answers"
                ));
            }
        }
    }

    // The preview domain's leftmost label comes from a PUSHED BRANCH NAME, and
    // `POST /api/webhooks/git/{id}/{secret}` has no auth extractor — so a repo
    // collaborator with no panel account chooses it. A branch called `www`, `mail`
    // or `api` used to synthesise `www.example.com` and hand it straight to the
    // agent, which replaced whatever vhost was already there.
    //
    // A collision here is almost always accidental, so the preview still deploys —
    // it just does not get a vhost. Losing a preview URL is a far smaller thing
    // than repointing a production site at a branch build. Note `is_reserved_domain`
    // and not `is_reserved_domain_for`: the Host header on this request is chosen
    // by whoever calls the webhook, so it must not be trusted to define what is
    // reserved.
    let mut preview_domain = config.domain.as_ref().map(|d| format!("{branch_slug}.{d}"));
    if let Some(ref candidate) = preview_domain {
        let taken = if is_reserved_domain(candidate) {
            Some(crate::services::domain_claim::Occupant::Site)
        } else {
            crate::services::domain_claim::find_occupant(
                &state.db,
                &state.agents,
                candidate,
                crate::services::domain_claim::Holder::GitDeploy(config.id),
            )
            .await
            .unwrap_or(None)
        };
        if taken.is_some() {
            tracing::warn!(
                "Preview for branch '{branch}' would have claimed {candidate}, which is \
                 already served on this server — deploying the preview without a domain"
            );
            preview_domain = None;
        }
    }

    // Upsert preview record.
    //
    // This is a refusal, not a warning, and the difference is the whole reason
    // the doc comment above can promise an honest answer. The row is the only
    // record that the container, the port, the vhost and the certificate created
    // below exist: every consumer reads `git_previews` (the previews list, Delete
    // Preview, both cleanup sweeps, the parent delete) and nothing reconciles
    // running containers against rows. Deploying past a failed write is how a
    // preview becomes unreapable — the same state :3081 already refuses to
    // create, for the same reason, forty lines up.
    if let Err(e) = sqlx::query(
        "INSERT INTO git_previews (git_deploy_id, server_id, branch, container_name, host_port, domain, status) \
         VALUES ($1, $2, $3, $4, $5, $6, 'deploying') \
         ON CONFLICT (git_deploy_id, branch) DO UPDATE SET status = 'deploying', server_id = $2, container_name = $4, host_port = $5, updated_at = NOW()"
    )
    .bind(config.id).bind(config.server_id).bind(branch).bind(&container_name).bind(port).bind(&preview_domain)
    .execute(&state.db).await
    {
        // The returned reason is echoed verbatim into the webhook's HTTP body at
        // :1684 and from there into GitHub's delivery log, and that door has no
        // auth extractor — so it carries no database detail. The detail goes to
        // the operator's journal instead.
        tracing::warn!("Failed to upsert git preview record for branch '{branch}': {e}");
        return Err(
            "the preview could not be recorded, so it was not deployed — retry the push".to_string(),
        );
    }

    // Spawn deploy task
    let db = state.db.clone();
    let agent = agent.clone();
    let name = config.name.clone();
    let repo_url = config.repo_url.clone();
    let dockerfile = config.dockerfile.clone();
    let build_args = config.build_args.clone();
    let build_context = config.build_context.clone();
    let container_port = config.container_port;
    let env_vars = config.env_vars.clone();
    let deploy_id = config.id;
    let key_path = config.deploy_key_path.clone();
    let ssl_email = config.ssl_email.clone();
    let branch = branch.to_string();

    tokio::spawn(async move {
        let branch_slug = dns_label(&branch);

        // Clone at preview branch
        let mut clone_body = serde_json::json!({
            "name": format!("{name}-pr-{branch_slug}"),
            "repo_url": repo_url,
            "branch": branch,
            "scope": "preview",
        });
        if let Some(ref kp) = key_path {
            clone_body["key_path"] = serde_json::json!(kp);
        }

        let clone_result = agent.post_long("/git/clone", Some(clone_body), 300).await;

        let commit_hash = match clone_result {
            Ok(r) => r.get("commit_hash").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
            Err(e) => {
                tracing::error!("Preview clone failed: {name}/{branch}: {e}");
                if let Err(db_err) = sqlx::query("UPDATE git_previews SET status = 'failed' WHERE git_deploy_id = $1 AND branch = $2")
                    .bind(deploy_id).bind(&branch).execute(&db).await
                {
                    tracing::warn!("Failed to update git preview status: {db_err}");
                }
                return;
            }
        };

        // Build
        let image_tag = match agent.post_long("/git/build", Some(serde_json::json!({
            "name": format!("{name}-pr-{branch_slug}"),
            "dockerfile": dockerfile,
            "commit_hash": commit_hash,
            "build_args": build_args,
            "build_context": build_context,
            "scope": "preview",
        })), 660).await {
            Ok(r) => r.get("image_tag").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
            Err(e) => {
                tracing::error!("Preview build failed: {name}/{branch}: {e}");
                if let Err(db_err) = sqlx::query("UPDATE git_previews SET status = 'failed' WHERE git_deploy_id = $1 AND branch = $2")
                    .bind(deploy_id).bind(&branch).execute(&db).await
                {
                    tracing::warn!("Failed to update git preview status: {db_err}");
                }
                return;
            }
        };

        // Deploy. memory_mb/cpu_percent are deliberately not inherited from the
        // parent app: preview containers have always run unbounded, and quietly
        // starting to cap them would be a behaviour change this fix did not set out
        // to make. Recorded rather than silently kept — see the s288 ledger.
        let deploy_body = build_deploy_body(DeployBody {
            name: &format!("{name}-pr-{branch_slug}"),
            image_tag: &image_tag,
            container_port,
            host_port: port,
            env_vars: &env_vars,
            domain: preview_domain.as_deref(),
            memory_mb: None,
            cpu_percent: None,
            // Pass SSL email so preview environments get HTTPS
            ssl_email: ssl_email.as_deref(),
            scope: "preview",
        });

        match agent.post_long("/git/deploy", Some(deploy_body), 120).await {
            Ok(result) => {
                let cid = result.get("container_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                // Record the name the AGENT says it created, not the one this
                // panel predicted. An agent older than this panel ignores the
                // scope it was sent and creates the unscoped name; storing the
                // prediction would leave the row pointing at nothing, and every
                // later teardown addressing a container that does not exist.
                let created = result
                    .get("container_name")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("dockpanel-git-{name}-pr-{branch_slug}"));
                if let Err(db_err) = sqlx::query("UPDATE git_previews SET status = 'running', container_id = $1, commit_hash = $2, container_name = $3 WHERE git_deploy_id = $4 AND branch = $5")
                    .bind(&cid).bind(&commit_hash).bind(&created).bind(deploy_id).bind(&branch).execute(&db).await
                {
                    tracing::warn!("Failed to update git preview status: {db_err}");
                }
                tracing::info!("Preview deployed: {name}/{branch} -> port {port}");
            }
            Err(e) => {
                tracing::error!("Preview deploy failed: {name}/{branch}: {e}");
                if let Err(db_err) = sqlx::query("UPDATE git_previews SET status = 'failed' WHERE git_deploy_id = $1 AND branch = $2")
                    .bind(deploy_id).bind(&branch).execute(&db).await
                {
                    tracing::warn!("Failed to update git preview status: {db_err}");
                }
            }
        }
    });

    Ok(())
}

/// GET /api/git-deploys/{id}/previews — List preview deployments.
pub async fn list_previews(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<GitPreview>>, ApiError> {
    require_admin(&claims.role)?;
    let previews: Vec<GitPreview> = sqlx::query_as(
        "SELECT p.* FROM git_previews p JOIN git_deploys g ON p.git_deploy_id = g.id WHERE g.id = $1 AND g.user_id = $2 ORDER BY p.created_at DESC LIMIT 500"
    ).bind(id).bind(claims.sub).fetch_all(&state.db).await
        .map_err(|e| internal_error("list previews", e))?;
    Ok(Json(previews))
}

/// DELETE /api/git-deploys/{id}/previews/{preview_id} — Delete a preview.
pub async fn delete_preview(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, preview_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    // `g.server_id` is joined in for the same reason `remove()` selects it: the
    // preview runs on the server its PARENT deployment lives on — that is the box
    // `handle_preview_deploy` built it on — and the caller's `ServerScope` was
    // deciding which agent got the teardown. A preview's container name is derived
    // from deploy name + branch, so two servers running the same repository produce
    // byte-identical preview names: aimed at the wrong host, `/git/cleanup` finds
    // the neighbour's live preview and destroys it, then deletes the row for the
    // one still running. The cleanup is fire-and-forget, so nothing reports it.
    let row: Option<(String, Option<String>, i32, Uuid)> = sqlx::query_as(
        "SELECT p.container_name, p.domain, p.host_port, g.server_id FROM git_previews p JOIN git_deploys g ON p.git_deploy_id = g.id WHERE p.id = $1 AND g.id = $2 AND g.user_id = $3"
    ).bind(preview_id).bind(id).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| internal_error("delete preview", e))?;

    let (container_name, preview_domain, host_port, server_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Preview not found"))?;

    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(server_id),
        preview_domain.as_deref().unwrap_or(&container_name),
    )
    .await?;

    // Clean up container — the stored name carries both the `dockpanel-git-`
    // prefix the agent re-adds and, for rows written from v2.55.0 on, the
    // preview scope. `preview_cleanup_target` resolves both.
    if let Err(e) = agent
        .post(
            "/git/cleanup",
            Some(preview_cleanup_body(
                &container_name,
                preview_domain.as_deref(),
                Some(host_port),
            )),
        )
        .await
    {
        // KEEP THE ROW and SAY SO. This door answered `{"ok": true}` over a
        // teardown that had failed, which is the worst of the four: the operator
        // is told it worked, so they never look again, and the row they would
        // have needed to try again with is already gone.
        //
        // The agent's own sentence travels only on a 4xx (`error.rs`), so take
        // it where there is one and name the transport failure where there is
        // not — and in both cases state that the preview was kept, because that
        // is what makes this retryable rather than just failed.
        tracing::warn!("Failed to tear down preview {container_name}: {e}");
        let unanswered = "the server did not answer".to_string();
        let (status, detail) =
            crate::error::agent_actionable(&e).unwrap_or((StatusCode::BAD_GATEWAY, unanswered));
        return Err(err(
            status,
            &format!(
                "Could not tear down preview {container_name}: {detail}. The preview was \
                 kept — its record is the only thing that can still find that container, \
                 its port and its vhost. Retry once the server answers."
            ),
        ));
    }

    if let Err(e) = sqlx::query("DELETE FROM git_previews WHERE id = $1").bind(preview_id).execute(&state.db).await {
        tracing::warn!("Failed to delete git preview record: {e}");
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/git-deploys/{id}/schedule — Schedule a one-time deploy.
pub async fn schedule_deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    // Verify ownership
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("schedule deploy", e))?;

    if existing.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Git deploy not found"));
    }

    let deploy_at = body.get("deploy_at")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "deploy_at is required (ISO 8601 timestamp)"))?;

    let scheduled_at = chrono::DateTime::parse_from_rfc3339(deploy_at)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid deploy_at format — use ISO 8601 (e.g., 2026-03-23T02:00:00Z)"))?;

    if scheduled_at <= chrono::Utc::now() {
        return Err(err(StatusCode::BAD_REQUEST, "deploy_at must be in the future"));
    }

    sqlx::query(
        "UPDATE git_deploys SET scheduled_deploy_at = $1, updated_at = NOW() WHERE id = $2"
    )
    .bind(scheduled_at)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("schedule deploy", e))?;

    tracing::info!("Scheduled one-time deploy for git deploy {id} at {scheduled_at}");
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "git_deploy.schedule",
        Some("git_deploy"), Some(&id.to_string()), Some(deploy_at), None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "scheduled_deploy_at": scheduled_at,
    })))
}

/// DELETE /api/git-deploys/{id}/schedule — Cancel a scheduled deploy.
pub async fn cancel_scheduled_deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("cancel scheduled deploy", e))?;

    if existing.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Git deploy not found"));
    }

    sqlx::query(
        "UPDATE git_deploys SET scheduled_deploy_at = NULL, updated_at = NOW() WHERE id = $1"
    )
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("cancel scheduled deploy", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Deploy Approvals ────────────────────────────────────────────────────────

/// GET /api/deploy-approvals — List pending deploy approvals.
pub async fn list_approvals(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    require_admin(&claims.role)?;

    // Scoped to the machines this administrator operates. `require_admin` alone is
    // not a boundary — every sibling read in this file also carries `user_id =
    // $claims.sub`, and this list carried nothing, so every pending approval in the
    // installation was legible to any admin account regardless of whose fleet the
    // deployment ran on. It leaks the deploy name and the requester's id, and it
    // feeds `approve_deploy` the ids to act on.
    //
    // The predicate cannot be the sibling one. `deploy()` is the only writer of
    // these rows and it fetches its config with `user_id = $2`, so `requested_by`
    // is ALWAYS the deployment's owner — and approving your own request is refused
    // two lines into `approve_deploy`. `g.user_id = $claims.sub` would therefore
    // make every approval unapprovable by construction: it would read as a tighter
    // guard while quietly deleting the feature.
    //
    // So the boundary is the one `SITE_CALLER_PREDICATE`'s admin arm already draws
    // for sites: an operator may act on the box in front of them — this machine, or
    // a server they registered themselves — and does not reach a machine somebody
    // else added. That keeps the two-person rule intact and still stops a second
    // tenant's admin from signing off on a deploy to hardware they do not run.
    // `u.role` is read from the DATABASE, not from `claims.role`, for the reason
    // stated there: a JWT keeps asserting the role it was minted with until it
    // expires, so a demoted account would otherwise keep approving all session.
    // `g.deploy_protected` is part of the predicate, not just of the write path.
    // A request survives its flag being switched off — `update()` now resolves
    // those, but an install that flipped the flag before this release still holds
    // them, and a row whose deployment is no longer protected must not be offered
    // for approval by a screen that shows the same deployment as unprotected.
    //
    // The requester's EMAIL and the deployment's repo/branch travel with the row
    // because a signature without them is not a review: `list()` scopes the deploy
    // table to its owner, so the approving administrator has usually never seen
    // this deployment anywhere else in the panel and would be authorising a
    // production build of a repository the screen never named. The email also
    // replaces nothing — the raw requester id was already on the wire — and it is
    // the less identifying of the two for an operator reading it.
    let rows: Vec<(Uuid, Uuid, Uuid, String, String, String, String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT da.id, da.deploy_id, da.requested_by, da.status, g.name, \
                u2.email, g.repo_url, g.branch, da.created_at \
         FROM deploy_approvals da \
         JOIN git_deploys g ON g.id = da.deploy_id \
         JOIN users u2 ON u2.id = da.requested_by \
         WHERE da.status = 'pending' AND g.deploy_protected AND EXISTS (\
             SELECT 1 FROM users u, servers sv WHERE u.id = $1 AND u.role = 'admin' \
             AND sv.id = g.server_id AND (sv.is_local OR sv.user_id = u.id)) \
         ORDER BY da.created_at DESC"
    )
    .bind(claims.sub)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list approvals", e))?;

    let result: Vec<serde_json::Value> = rows.into_iter()
        .map(|(id, deploy_id, requested_by, status, name, requested_by_email, repo_url, branch, created_at)| {
            serde_json::json!({
                "id": id,
                "deploy_id": deploy_id,
                "requested_by": requested_by,
                "requested_by_email": requested_by_email,
                "status": status,
                "deploy_name": name,
                // This list is the ONE place a git deploy's URL crosses an
                // ownership boundary: the reader is an administrator of the
                // machine, not the operator who configured the deploy. Masking
                // it in the SPA would not be enough, because the token would
                // still be in this response body. Nothing edits a repo URL from
                // the approval queue, so the masked form is all it ever needed.
                "repo_url": mask_repo_credentials(&repo_url),
                "branch": branch,
                "created_at": created_at,
            })
        }).collect();

    Ok(Json(result))
}

/// POST /api/deploy-approvals/{id}/approve — Approve a pending deploy.
pub async fn approve_deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(approval_id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    // Fetch the pending approval, scoped to the machines this administrator
    // operates — the same predicate `list_approvals` now applies, and the reason it
    // cannot simply be `g.user_id = $claims.sub` is written out in full there.
    // Unscoped, this handler let any admin account sign off on a protected deploy
    // belonging to a different tenant on hardware they do not run, which is exactly
    // the review the `deploy_protected` flag exists to force.
    let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT da.deploy_id, da.requested_by, da.status \
         FROM deploy_approvals da \
         JOIN git_deploys g ON g.id = da.deploy_id \
         WHERE da.id = $1 AND EXISTS (\
             SELECT 1 FROM users u, servers sv WHERE u.id = $2 AND u.role = 'admin' \
             AND sv.id = g.server_id AND (sv.is_local OR sv.user_id = u.id))"
    )
    .bind(approval_id)
    .bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("approve deploy", e))?;

    let (deploy_id, requested_by, status) = row
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Approval not found"))?;

    if status != "pending" {
        return Err(err(StatusCode::CONFLICT, &format!("Approval already {status}")));
    }

    // Cannot approve your own deploy
    if requested_by == claims.sub {
        return Err(err(StatusCode::FORBIDDEN, "Cannot approve your own deploy request"));
    }

    // Load config and resolve the host BEFORE consuming the approval.
    //
    // The order matters and it changed with this fix. The agent used to arrive as
    // an extractor, so an unreachable host was answered by `ServerScope` with a 502
    // before the handler body ran and the approval stayed pending, retryable.
    // Resolving it here — correctly, from the row — moves that failure INSIDE the
    // body, and marking the approval first would burn it: `status` is no longer
    // 'pending', so the retry is refused with 409 and a protected deploy needs a
    // fresh request and a second admin all over again because a box was rebooting.
    //
    // The config load is pinned to `requested_by`, which is the owner `deploy()`
    // verified when it wrote the approval row. If the deployment changed hands in
    // between, this fails closed rather than deploying on the strength of a request
    // its current owner never made.
    let config: GitDeploy = sqlx::query_as(
        "SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2"
    )
    .bind(deploy_id)
    .bind(requested_by)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("approve deploy", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;

    // The flag is re-read here, not assumed from the row's existence. `deploy()`
    // checked it when the request was filed, and an owner may have cleared it
    // since; `update()` now cancels those requests, but an install that flipped
    // the flag before this release still holds them. Approving one would deploy a
    // deployment nobody currently requires review for, on the strength of a review
    // requirement that no longer exists.
    if !config.deploy_protected {
        return Err(err(
            StatusCode::CONFLICT,
            "Deploy protection is no longer enabled on this deployment; the request is obsolete",
        ));
    }

    // The build runs on the server the deployment lives on — see `deploy()`, of
    // which this is the deferred half. It carried the sharper version of the bug:
    // the requester and the approver are different people, so the approver's server
    // switcher decided where somebody else's deploy landed.
    let agent = crate::helpers::agent_for_site_server(
        &state,
        Some(config.server_id),
        config.domain.as_deref().unwrap_or(&config.name),
    )
    .await?;

    // The SAME atomic deploy lock `deploy()` takes, and taken BEFORE the approval
    // is consumed, for the reason the comment above already gives for the agent:
    // a losing caller must leave the request retryable rather than burn it.
    //
    // This path used to flip the status unconditionally and merely warn on error,
    // so two administrators resolving the queue at the same moment — or an
    // approval landing while an unprotected sibling deploy was mid-build — each
    // spawned a full production build against one working tree. The condition is
    // copied from `deploy()` verbatim, including the 30-minute self-heal for a
    // crashed build, so the two doors cannot drift into disagreeing about what
    // "already in progress" means.
    match sqlx::query(
        "UPDATE git_deploys SET status = 'building', updated_at = NOW() \
         WHERE id = $1 AND (status IS DISTINCT FROM 'building' OR updated_at < NOW() - INTERVAL '30 minutes')"
    ).bind(deploy_id).execute(&state.db).await {
        Ok(r) if r.rows_affected() == 0 => return Err(err(StatusCode::CONFLICT, "Deploy already in progress")),
        Ok(_) => {}
        Err(e) => return Err(internal_error("deploy lock", e)),
    }

    // Mark as approved. After the lock, so a refused approval stays pending.
    sqlx::query(
        "UPDATE deploy_approvals SET status = 'approved', approved_by = $1, resolved_at = NOW() WHERE id = $2"
    )
    .bind(claims.sub).bind(approval_id)
    .execute(&state.db).await
    .map_err(|e| internal_error("approve deploy", e))?;

    let new_deploy_id = Uuid::new_v4();
    // The approver, not the requester — and deliberately so. `new_deploy_id`
    // is returned in this response and stored nowhere else, so the approver is
    // the only party that ever learns it; the requester got a bare
    // "pending_approval" with no id and has nothing to open a stream with.
    // Recording the requester as owner would name someone who cannot ask and
    // refuse the only one who can, leaving the log readable by nobody.
    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        new_deploy_id,
        claims.sub,
        32,
    );

    spawn_deploy_task(
        state,
        agent,
        new_deploy_id,
        config,
        requested_by,
        claims.email,
        "approved",
    );

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "status": "approved",
        "deploy_id": new_deploy_id,
        "message": "Deploy approved and started",
    }))))
}

/// POST /api/deploy-approvals/{id}/reject — Reject a pending deploy.
pub async fn reject_deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(approval_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    // Same boundary as `list_approvals` and `approve_deploy`, for the same reason:
    // an approval this administrator cannot see must not be one they can resolve.
    // Rejection has no agent and so no wrong-host half, but unscoped it is the
    // denial-of-service twin of approve — a second tenant's admin could kill any
    // protected deploy in the installation, and rejection is terminal (the check
    // below refuses anything not 'pending', so the requester must start over).
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT da.status FROM deploy_approvals da \
         JOIN git_deploys g ON g.id = da.deploy_id \
         WHERE da.id = $1 AND EXISTS (\
             SELECT 1 FROM users u, servers sv WHERE u.id = $2 AND u.role = 'admin' \
             AND sv.id = g.server_id AND (sv.is_local OR sv.user_id = u.id))"
    )
    .bind(approval_id)
    .bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("reject deploy", e))?;

    let (status,) = row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Approval not found"))?;

    if status != "pending" {
        return Err(err(StatusCode::CONFLICT, &format!("Approval already {status}")));
    }

    sqlx::query(
        "UPDATE deploy_approvals SET status = 'rejected', approved_by = $1, resolved_at = NOW() WHERE id = $2"
    )
    .bind(claims.sub).bind(approval_id)
    .execute(&state.db).await
    .map_err(|e| internal_error("reject deploy", e))?;

    Ok(Json(serde_json::json!({
        "status": "rejected",
        "message": "Deploy request rejected",
    })))
}

#[cfg(test)]
mod tests {
    use super::{
        DeployBody, blank_to_none, build_deploy_body, env_object, is_valid_cron,
        mask_repo_credentials, preview_cleanup_target, strip_container_prefix,
    };

    #[test]
    fn an_embedded_credential_never_reaches_a_reader_who_does_not_own_it() {
        // The workaround operators reach for when a private HTTPS clone fails,
        // and the reason #119 was filed. Both spellings git accepts.
        assert_eq!(
            mask_repo_credentials("https://ghp_secret123@github.com/me/app.git"),
            "https://•••@github.com/me/app.git"
        );
        assert_eq!(
            mask_repo_credentials("https://user:ghp_secret123@github.com/me/app.git"),
            "https://•••@github.com/me/app.git"
        );
        // TWO '@' inside the authority. A username may legitimately contain one
        // (Azure DevOps and some LDAP-backed hosts issue them), and the
        // authority boundary is the LAST one — splitting on the first leaves
        // ":tok@" on screen, i.e. the secret this function exists to hide. This
        // is the case that distinguishes rfind from find; without it the test
        // passes under both and certifies a leak.
        assert_eq!(
            mask_repo_credentials("https://me@corp.com:ghp_secret123@github.com/me/app.git"),
            "https://•••@github.com/me/app.git"
        );
        // A URL with nothing to hide is returned untouched — the operator still
        // has to be able to tell which repository a row is for.
        assert_eq!(
            mask_repo_credentials("https://github.com/me/app.git"),
            "https://github.com/me/app.git"
        );
        // An '@' in the PATH is not userinfo. Splitting on the first '@' instead
        // of the authority would have masked the host out of this one.
        assert_eq!(
            mask_repo_credentials("https://git.example.com/~user@host/app.git"),
            "https://git.example.com/~user@host/app.git"
        );
        // scp-style SSH remotes carry no scheme and no secret; leave them alone.
        assert_eq!(
            mask_repo_credentials("git@github.com:me/app.git"),
            "git@github.com:me/app.git"
        );
    }

    #[test]
    fn a_blank_field_is_stored_as_null_not_as_an_empty_string() {
        // NULL is this table's spelling of "not set", and `remove` reads the
        // domain column with `unwrap_or(&name)` — a stored "" would hand the
        // agent an empty site identifier where NULL correctly yields the name.
        assert_eq!(blank_to_none(Some("")), None);
        assert_eq!(blank_to_none(Some("   ")), None);
        assert_eq!(blank_to_none(None), None);
        assert_eq!(
            blank_to_none(Some("prisma-migrate")),
            Some("prisma-migrate")
        );
    }

    #[test]
    fn a_preview_is_torn_down_in_the_space_it_was_created_in() {
        // Written from v2.55.0 on: scoped, so the agent addresses the preview's
        // own space and refuses anything labelled as a deployment.
        assert_eq!(
            preview_cleanup_target("dockpanel-git-pr.myapp-pr-feature-x"),
            ("myapp-pr-feature-x".to_string(), "preview")
        );
        // Written before it: the old shared space. Addressed by the old name —
        // recomputing it would be a second answer to a question the row already
        // holds — but with the ownership rule that refuses a labelled deploy.
        assert_eq!(
            preview_cleanup_target("dockpanel-git-myapp-pr-feature-x"),
            ("myapp-pr-feature-x".to_string(), "preview_legacy")
        );
    }

    #[test]
    fn the_two_stored_shapes_are_never_the_same_string() {
        // The collision this replaced: the legacy preview of config `myapp` on
        // branch `feature-x` WAS the container of a deployment named
        // `myapp-pr-feature-x`, which `is_valid_name` accepts.
        let legacy = "dockpanel-git-myapp-pr-feature-x";
        let scoped = format!("dockpanel-git-{}myapp-pr-feature-x", super::PREVIEW_SCOPE_PREFIX);
        assert_ne!(legacy, scoped);
        assert!(!crate::routes::is_valid_name(strip_container_prefix(&scoped)));
        assert!(crate::routes::is_valid_name(strip_container_prefix(legacy)));
    }

    fn body_with(env: serde_json::Value) -> serde_json::Value {
        build_deploy_body(DeployBody {
            name: "app",
            image_tag: "dockpanel-git-app:abc",
            container_port: 3000,
            host_port: 30001,
            env_vars: &env,
            domain: None,
            memory_mb: None,
            cpu_percent: None,
            ssl_email: None,
            scope: "deploy",
        })
    }

    #[test]
    fn deploy_body_sends_the_key_the_agent_reads() {
        // GH #94: the panel spelled this "env_vars" and the agent's DeployRequest
        // declares "env" with serde(default), so the environment was dropped and
        // the deploy still reported success.
        let body = body_with(serde_json::json!({ "APP_KEY": "secret" }));
        assert_eq!(body["env"]["APP_KEY"], "secret");
        // Sending both spellings is worse than sending the wrong one: serde treats
        // a field plus its alias as a duplicate field and rejects the request.
        assert!(body.get("env_vars").is_none(), "must not also send the alias");
    }

    #[test]
    fn optional_fields_are_omitted_not_nulled() {
        let body = body_with(serde_json::json!({}));
        for k in ["domain", "memory_mb", "cpu_percent", "ssl_email"] {
            assert!(body.get(k).is_none(), "{k} should be absent, not null");
        }
    }

    #[test]
    fn env_scalars_are_stringified_rather_than_failing_the_deploy() {
        // env_vars is an unconstrained JSONB column. The agent reads it as
        // HashMap<String, String>, so one numeric value in one row would other-
        // wise reject the WHOLE deploy — trading a bug that lost the environment
        // for a bug that refuses to deploy at all.
        let env = env_object(&serde_json::json!({
            "PORT": 8080,
            "DEBUG": true,
            "NAME": "app",
        }));
        assert_eq!(env["PORT"], "8080");
        assert_eq!(env["DEBUG"], "true");
        assert_eq!(env["NAME"], "app");
    }

    #[test]
    fn env_values_with_no_env_representation_are_dropped() {
        let env = env_object(&serde_json::json!({
            "GOOD": "keep",
            "NULLED": serde_json::Value::Null,
            "NESTED": { "a": 1 },
            "LIST": [1, 2],
        }));
        assert_eq!(env["GOOD"], "keep");
        for k in ["NULLED", "NESTED", "LIST"] {
            assert!(env.get(k).is_none(), "{k} should be dropped");
        }
    }

    #[test]
    fn a_non_object_env_column_yields_an_empty_map() {
        assert_eq!(env_object(&serde_json::Value::Null), serde_json::json!({}));
        assert_eq!(env_object(&serde_json::json!("oops")), serde_json::json!({}));
    }

    #[test]
    fn strip_prefix_from_stored_preview_name() {
        // git_previews.container_name is stored WITH the prefix; cleanup must strip it
        // so the agent (which re-adds "dockpanel-git-") resolves the real container.
        assert_eq!(strip_container_prefix("dockpanel-git-myapp-pr-feat-x"), "myapp-pr-feat-x");
    }

    #[test]
    fn strip_prefix_is_idempotent_and_safe_on_bare() {
        // A bare name (the manual path already passes this) is returned unchanged.
        assert_eq!(strip_container_prefix("myapp-pr-feat-x"), "myapp-pr-feat-x");
        // Only ONE prefix is stripped — a double-prefixed input still loses just one,
        // but the stored value only ever carries a single prefix.
        assert_eq!(strip_container_prefix("dockpanel-git-dockpanel-git-x"), "dockpanel-git-x");
    }

    #[test]
    fn valid_cron_accepts_what_the_scheduler_runs() {
        // Must NOT be stricter than deploy_scheduler::matches_field, else editing a
        // deploy whose stored cron the scheduler already fires gets rejected.
        for c in [
            "* * * * *", "*/5 * * * *", "0 3 * * *", "0 0 1,15 * *", "0 9-17 * * 1-5",
            "* * * * * *",   // 6-field: scheduler reads first 5 + ignores extras
            "*/5,30 * * * *", // comma list mixing a step and a number
            "1,,5 * * * *",   // empty comma segment: scheduler skips it, so don't reject
        ] {
            assert!(is_valid_cron(c), "{c} should be valid");
        }
    }

    #[test]
    fn valid_cron_rejects_malformed() {
        for c in ["", "* * * *", "abc * * * *", "*/0 * * * *", "@daily"] {
            assert!(!is_valid_cron(c), "{c} should be rejected");
        }
    }

    #[test]
    fn dns_label_makes_valid_preview_slugs() {
        assert_eq!(super::dns_label("feature_login"), "feature-login"); // '_' would fail domain validation
        assert_eq!(super::dns_label("feature/JIRA-42"), "feature-jira-42");
        assert_eq!(super::dns_label("--weird__"), "weird");
        assert_eq!(super::dns_label("###"), "preview"); // fallback when nothing alphanumeric remains
        assert_eq!(super::dns_label("Main"), "main");
    }
}
