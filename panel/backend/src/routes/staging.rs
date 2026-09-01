use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::auth::Claims;
use crate::error::{internal_error, err, agent_error, ApiError};
use crate::models::Site;
use crate::services::activity;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct CreateStagingRequest {
    /// Custom staging domain (optional). Defaults to staging.{parent_domain}.
    pub domain: Option<String>,
    /// Explicit opt-in past the shared-database warning below. Defaults to
    /// false/absent so the warning is the default outcome for any site that
    /// has one, not an opt-out a caller has to know to avoid.
    #[serde(default)]
    pub acknowledge_shared_database: bool,
}

/// Helper: resolve a site this caller may act on, as a full row.
///
/// Shares [`crate::helpers::SITE_CALLER_PREDICATE`] with every other per-site
/// read, rather than keeping this module's own copy of an owner-only predicate.
async fn get_site(state: &AppState, id: Uuid, claims: &Claims) -> Result<Site, ApiError> {
    sqlx::query_as::<_, Site>(&format!("SELECT s.* FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE))
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("resolve site for caller", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))
}

/// Resolve the one agent that can move files between a site and its staging environment.
///
/// `/staging/sync` is an rsync from `/var/www/{source}` into `/var/www/{target}` on whichever
/// host is asked, so the operation only exists on a machine that holds both paths. Each row
/// names its own host, and the pair has to agree.
///
/// `create` now inherits the parent's `server_id`, so nothing here makes a mismatched pair any
/// more — but it made them for as long as it stamped the row with the caller's scope, and
/// those rows are still in every database that ran that version. This is the guard that
/// catches them, and it stays after the source is fixed: an invariant nothing enforces at
/// write time is an invariant that comes back.
///
/// When the two disagree there is nothing to choose between — one of the paths is not there.
/// Refusing says so; substituting either host would rsync one machine's document root into a
/// directory belonging to a site that is not the one being copied. The mismatch is reported as
/// a conflict rather than a not-found because the data is inconsistent, not absent.
async fn same_host_agent(
    state: &AppState,
    parent: &Site,
    staging: &Site,
) -> Result<crate::services::agent::AgentHandle, ApiError> {
    if parent.server_id != staging.server_id {
        tracing::warn!(
            "Refusing to move files between {} and {}: their rows name different servers \
             ({:?} vs {:?})",
            parent.domain,
            staging.domain,
            parent.server_id,
            staging.server_id
        );
        return Err(err(
            StatusCode::CONFLICT,
            "This site and its staging environment are on different servers",
        ));
    }

    crate::helpers::agent_for_site_server(state, staging.server_id, &staging.domain).await
}

/// POST /api/sites/{id}/staging — Create a staging environment.
///
/// 1. Validate parent site (must be active, not already a staging site)
/// 2. Generate or validate staging domain
/// 3. Create site record with parent_site_id
/// 4. Create nginx config via agent
/// 5. Clone files from production
///
/// Takes no `ServerScope`, and that is NOT the usual exception for a handler that creates a
/// row. `sites::create` and `stacks::create` legitimately read the host from the caller,
/// because a brand-new site or stack has a free choice of machine and the caller's selection
/// is the only thing that expresses it. A staging environment has no such choice: it exists
/// to be rsynced to and from its parent's document root, and an rsync is one machine's work.
/// Its host is therefore a property of the parent, not a decision this request gets to make —
/// so it is inherited, not read from the header.
///
/// This is where the divergence `same_host_agent` refuses used to be manufactured: the row
/// was stamped with the caller's scope, so creating staging for a site on host B while
/// scoped to host A recorded a staging environment on A, cloned production's files onto A,
/// and left every later sync and push with two rows naming two machines.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(body): Json<CreateStagingRequest>,
) -> Result<(StatusCode, Json<Site>), ApiError> {
    // A staging environment is a `sites` row. The panel has four handlers that
    // write one, and the admission controls the other three enforce were written
    // into `sites::clone_site` with a comment stating the rule in general terms —
    // that a handler creating a site "must enforce the SAME admission controls
    // create() does — otherwise it is a create() with none of the guards". That
    // fix reached the sibling in its own file and stopped there.
    //
    // So this door stayed open. The per-site uniqueness check below bounds a
    // tenant to one staging environment per parent, which is why this never read
    // as mass creation — but the bound is per parent, not per tenant, so an
    // account holding N sites could mint N more rows, each with a vhost and a
    // cloned document root, during a declared lockdown.
    //
    // Both guards are the sibling's own: the same predicate, the same status, and
    // the shared ceiling rather than a second copy of the number.
    if crate::services::security_hardening::is_locked_down(&state.db).await {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "System is in lockdown mode"));
    }
    {
        let max_sites: i64 = crate::routes::sites::site_rate_limit(&state.db).await;
        let recent: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sites WHERE user_id = $1 AND created_at > NOW() - INTERVAL '1 hour'",
        )
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));
        if recent.0 >= max_sites {
            let _ = crate::services::security_hardening::record_suspicious_event(
                &state.db,
                "site.rate_limit_hit",
                Some(&claims.email),
                None,
                Some(&format!(
                    "User tried to create staging site #{} in 1 hour",
                    recent.0 + 1
                )),
            )
            .await;
            return Err(err(
                StatusCode::TOO_MANY_REQUESTS,
                &format!("Site creation rate limit: max {max_sites} sites per hour. \
                          An administrator can change this in Settings > Account > Security Hardening \
                          (\"Site Creation Rate Limit\"); set it to 0 to remove the limit."),
            ));
        }
    }

    let parent = get_site(&state, id, &claims).await?;

    if parent.status != "active" {
        return Err(err(StatusCode::BAD_REQUEST, "Parent site must be active"));
    }
    if parent.parent_site_id.is_some() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Cannot create staging from a staging site",
        ));
    }

    // Staging only ever clones FILES (`/staging/clone` is an rsync of
    // `/var/www/{domain}`, confirmed by reading the full agent-side staging
    // surface — there is no database keyword anywhere in it). For a
    // WordPress/Laravel/any DB-backed site, that means the staging environment
    // reads and writes the SAME live production database from the moment it's
    // created — not a copy, not deferred until "Push to Prod", immediately.
    // The UI copy above this form used to promise a safe place to "test
    // changes before going live" with nothing disclosing that the database
    // half of that promise is false. Block by default rather than silently
    // let an operator corrupt production data through what looks like a
    // sandbox; `acknowledge_shared_database` is the explicit, informed
    // override for the sites this is actually fine for (WordPress plugin/theme
    // testing where no destructive DB writes are expected, etc).
    if !body.acknowledge_shared_database {
        let has_database: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM databases WHERE site_id = $1 LIMIT 1")
                .bind(id)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| internal_error("create staging", e))?;
        if has_database.is_some() {
            return Err(err(
                StatusCode::CONFLICT,
                "This site has an attached database. Staging only clones files — the \
                 database is NOT isolated, so changes made on staging read and write the \
                 LIVE production database immediately, not a copy.",
            ));
        }
    }

    // Check if staging already exists for this site
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM sites WHERE parent_site_id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("create staging", e))?;

    if existing.is_some() {
        return Err(err(
            StatusCode::CONFLICT,
            "A staging environment already exists for this site",
        ));
    }

    // The parent's host, for the rest of this handler: the claim below, the vhost, the file
    // clone and the `server_id` written on the new row all have to be the same machine, and
    // that machine is the one production is already on. `server_id` is `Option` on the struct
    // and `NOT NULL` in the schema, so a row predating the backfill would otherwise be bound
    // as NULL and fail on the INSERT — refuse it here, with the reason, instead.
    let server_id = parent.server_id.ok_or_else(|| {
        tracing::warn!(
            "Refusing to create staging for {}: its row names no server",
            parent.domain
        );
        err(
            StatusCode::CONFLICT,
            "This site is not associated with a server",
        )
    })?;
    let agent =
        crate::helpers::agent_for_site_server(&state, Some(server_id), &parent.domain).await?;

    // Determine staging domain. `body.domain` is arbitrary — it is NOT required to
    // be a subdomain of the parent — so this is a full domain claim by an ordinary
    // tenant, and it must pass the same guard as every other one. Until now it
    // consulted the `sites` table alone: no reserved-domain block (so a tenant who
    // owned one site could claim the panel's own hostname), no git deploys, and no
    // Docker apps.
    let requested = match body.domain {
        Some(ref d) if !d.is_empty() => d.clone(),
        _ => format!("staging.{}", parent.domain),
    };
    let staging_domain = crate::services::domain_claim::ensure_claimable(
        &state.db,
        &state.agents,
        &requested,
        &headers,
        crate::services::domain_claim::Holder::New,
        &claims.role,
    )
    .await?;

    // Staging always lands on the parent's own server (`same_host_agent`'s whole
    // premise), and `idx_sites_proxy_port_server` is UNIQUE on `(proxy_port,
    // server_id)` — so binding `parent.proxy_port` verbatim onto a row that
    // shares the parent's `server_id` collides with the parent's OWN row every
    // time, deterministically, for every proxy/node/python-runtime site. This
    // was diagnosed and shipped as a migration (`20260818000000_port_uniqueness
    // _server_scope.sql`) the same day the bug was first found, but the fix
    // here — allocating staging its own port instead of copying the parent's —
    // was never made, so staging a non-static-non-php site has 500'd on this
    // exact constraint ever since. Allocated the same way `sites::create`
    // auto-allocates for node/python (`generate_series(5000, 5999)` scoped to
    // this server), since staging never takes a caller-supplied port.
    let staging_proxy_port = if parent.proxy_port.is_some() {
        let row: Option<(i32,)> = sqlx::query_as(
            "SELECT s.port FROM generate_series(5000, 5999) AS s(port) \
             WHERE s.port NOT IN (SELECT proxy_port FROM sites WHERE proxy_port IS NOT NULL AND server_id = $1) \
             LIMIT 1",
        )
        .bind(server_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("create staging", e))?;
        Some(row.map(|(p,)| p).ok_or_else(|| {
            err(StatusCode::CONFLICT, "No free port available for this staging environment on this server")
        })?)
    } else {
        None
    };

    // Insert staging site
    let staging: Site = sqlx::query_as(
        "INSERT INTO sites (user_id, server_id, domain, runtime, status, proxy_port, php_version, app_command, parent_site_id) \
         VALUES ($1, $2, $3, $4, 'creating', $5, $6, $7, $8) RETURNING *",
    )
    .bind(claims.sub)
    .bind(server_id)
    .bind(&staging_domain)
    .bind(&parent.runtime)
    .bind(staging_proxy_port)
    .bind(&parent.php_version)
    .bind(&parent.app_command)
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        // `idx_sites_parent_unique` is what actually adjudicates the race the
        // SELECT-then-INSERT check above cannot: two concurrent requests can
        // both see "none exists" and both reach this INSERT, and only one of
        // them lands. Named distinctly so the loser sees the same, correct
        // "already exists" message the check above gives the common case,
        // rather than a generic 500.
        if e.to_string().contains("idx_sites_parent_unique") {
            err(StatusCode::CONFLICT, "A staging environment already exists for this site")
        } else {
            internal_error("create staging", e)
        }
    })?;

    // Build agent request to create nginx config (same as regular site creation)
    let mut agent_body = serde_json::json!({
        "runtime": parent.runtime,
    });
    if let Some(port) = staging_proxy_port {
        agent_body["proxy_port"] = serde_json::json!(port);
    }
    if let Some(ref php) = parent.php_version {
        agent_body["php_socket"] =
            serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
    }
    // Without this, node/python staging silently got the same "app service
    // never starts" bug as the migration importer: `create_app_service`
    // (agent's `put_site`) requires BOTH `app_command` AND `proxy_port` to be
    // present, and this field was never sent at all.
    if let Some(ref cmd) = parent.app_command {
        agent_body["app_command"] = serde_json::json!(cmd);
    }

    let agent_path = format!("/nginx/sites/{}", staging_domain);
    if let Err(e) = agent.put(&agent_path, agent_body).await {
        tracing::error!("Agent error creating staging site {staging_domain}: {e}");
        sqlx::query("UPDATE sites SET status = 'error', updated_at = NOW() WHERE id = $1")
            .bind(staging.id)
            .execute(&state.db)
            .await
            .ok();
        return Err(agent_error("Staging configuration", e));
    }

    // Clone files from production to staging
    let clone_result = agent
        .post(
            "/staging/clone",
            Some(serde_json::json!({
                "source": parent.domain,
                "target": staging_domain,
            })),
        )
        .await;

    let synced_at = if clone_result.is_ok() {
        Some("NOW()")
    } else {
        tracing::warn!("File clone failed for staging {staging_domain}: {:?}", clone_result);
        None
    };

    // Status reflects whether the clone actually happened. Both branches used
    // to write 'active' regardless — the only difference was whether
    // `synced_at` got stamped, so a failed clone looked identical to a working
    // one everywhere except that one timestamp field. `status = 'error'` here
    // matches the pattern the `agent.put` failure branch above already uses.
    let update_sql = if synced_at.is_some() {
        "UPDATE sites SET status = 'active', synced_at = NOW(), updated_at = NOW() WHERE id = $1 RETURNING *"
    } else {
        "UPDATE sites SET status = 'error', updated_at = NOW() WHERE id = $1 RETURNING *"
    };

    let updated: Site = sqlx::query_as(update_sql)
        .bind(staging.id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("create staging", e))?;

    tracing::info!(
        "Staging created: {} → {} ({})",
        parent.domain,
        staging_domain,
        parent.runtime
    );
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "staging.create",
        Some("site"),
        Some(&staging_domain),
        Some(&parent.domain),
        None,
    )
    .await;

    Ok((StatusCode::CREATED, Json(updated)))
}

/// GET /api/sites/{id}/staging — Get staging site for a production site.
///
/// Takes no `ServerScope`: the staging row names its own host, and that is the only
/// host whose disk this may be measured on.
pub async fn get_staging(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify parent site ownership
    let _parent = get_site(&state, id, &claims).await?;

    let staging: Option<Site> =
        sqlx::query_as("SELECT * FROM sites WHERE parent_site_id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("get staging", e))?;

    match staging {
        Some(s) => {
            // A disk-usage figure is a measurement of one specific machine. Read through
            // the caller's handle it measured whichever host the request happened to be
            // scoped to — on a fleet, some other box's `/var/www/{domain}`, reported as
            // this environment's size with nothing in the answer to say otherwise. A wrong
            // number here is worse than no number, because a wrong number is believed.
            let agent =
                crate::helpers::agent_for_site_server(&state, s.server_id, &s.domain).await?;

            // Get disk usage
            let usage = agent
                .post(
                    "/staging/disk-usage",
                    Some(serde_json::json!({ "domain": s.domain })),
                )
                .await
                .ok()
                .and_then(|v| v.get("bytes").and_then(|b| b.as_u64()))
                .unwrap_or(0);

            Ok(Json(serde_json::json!({
                "exists": true,
                "site": s,
                "disk_usage_bytes": usage,
            })))
        }
        None => Ok(Json(serde_json::json!({ "exists": false }))),
    }
}

/// POST /api/sites/{id}/staging/sync — Sync production files → staging.
///
/// Takes no `ServerScope`: both directories in this copy are named by rows, and the rows
/// decide which machine holds them.
pub async fn sync_to_staging(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let parent = get_site(&state, id, &claims).await?;

    let staging: Site =
        sqlx::query_as("SELECT * FROM sites WHERE parent_site_id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("sync to staging", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "No staging environment found"))?;

    // Production is the source and staging is the target, and both paths have to exist on
    // the host that does the copying.
    let agent = same_host_agent(&state, &parent, &staging).await?;

    agent
        .post(
            "/staging/sync",
            Some(serde_json::json!({
                "source": parent.domain,
                "target": staging.domain,
            })),
        )
        .await
        .map_err(|e| agent_error("Staging sync", e))?;

    // Update synced_at timestamp
    sqlx::query("UPDATE sites SET synced_at = NOW(), updated_at = NOW() WHERE id = $1")
        .bind(staging.id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("sync to staging", e))?;

    tracing::info!("Synced {} → {}", parent.domain, staging.domain);
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "staging.sync",
        Some("site"),
        Some(&staging.domain),
        Some(&format!("{} → {}", parent.domain, staging.domain)),
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "message": format!("Synced {} → {}", parent.domain, staging.domain) })))
}

/// POST /api/sites/{id}/staging/push — Push staging files → production.
///
/// Takes no `ServerScope`: the rows decide the host, and this direction writes over a live
/// production document root, so the host had better be the right one.
pub async fn push_to_prod(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let parent = get_site(&state, id, &claims).await?;

    let staging: Site =
        sqlx::query_as("SELECT * FROM sites WHERE parent_site_id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("push to prod", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "No staging environment found"))?;

    // Same pairing as the sync above, reversed: staging is the source, production the target.
    let agent = same_host_agent(&state, &parent, &staging).await?;

    agent
        .post(
            "/staging/sync",
            Some(serde_json::json!({
                "source": staging.domain,
                "target": parent.domain,
            })),
        )
        .await
        .map_err(|e| agent_error("Staging push", e))?;

    tracing::info!("Pushed {} → {}", staging.domain, parent.domain);
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "staging.push",
        Some("site"),
        Some(&staging.domain),
        Some(&format!("{} → {}", staging.domain, parent.domain)),
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "message": format!("Pushed {} → {}", staging.domain, parent.domain) })))
}

/// DELETE /api/sites/{id}/staging — Delete the staging environment.
///
/// Takes no `ServerScope`. The caller's selection had no business here at all: `get_site`
/// authorises the PARENT, and the row this then deletes is a different one that carries its
/// own `server_id`.
pub async fn destroy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _parent = get_site(&state, id, &claims).await?;

    let staging: Site =
        sqlx::query_as("SELECT * FROM sites WHERE parent_site_id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("destroy", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "No staging environment found"))?;

    // The two calls below are a vhost removal and `/staging/delete-files`, which on the agent
    // is `remove_dir_all("/var/www/{domain}")` with no ownership check of its own — the agent
    // deletes the path it is handed, on the host it is asked. That makes the panel the only
    // thing that decides which machine loses a document root, and it was deciding from the
    // caller's request header while the authority sat unread in the row it had just loaded.
    // On a fleet, a delete of a staging environment reached whichever box the operator's
    // scope pointed at, and reported success either way: the DB row goes regardless, so the
    // record of what should have been deleted disappears along with the wrong directory.
    let agent =
        crate::helpers::agent_for_site_server(&state, staging.server_id, &staging.domain).await?;

    // Remove nginx config
    let agent_path = format!("/nginx/sites/{}", staging.domain);
    agent
        .delete(&agent_path)
        .await
        .map_err(|e| agent_error("Staging removal", e))?;

    // Delete site files. The row deleted below is the ONLY thing that marks
    // this domain as occupied — `domain_claim::find_occupant` checks the DB,
    // never the filesystem — so on failure the row must NOT be deleted, or the
    // domain becomes immediately re-claimable by anyone while the previous
    // tenant's files (potentially `wp-config.php`, `.env`, or other
    // credential-bearing files) are still on disk, and nothing that provisions
    // a new site or staging environment here clears the directory first. This
    // used to be a `.ok()` best-effort call with the DELETE below running
    // unconditionally afterward regardless of the outcome.
    if let Err(e) = agent
        .post(
            "/staging/delete-files",
            Some(serde_json::json!({ "domain": staging.domain })),
        )
        .await
    {
        tracing::error!(
            "Staging destroy: file cleanup failed for {} — keeping the site row so the \
             domain stays claimed rather than releasing it over leftover files: {e}",
            staging.domain
        );
        let _ = sqlx::query("UPDATE sites SET status = 'error', updated_at = NOW() WHERE id = $1")
            .bind(staging.id)
            .execute(&state.db)
            .await;
        return Err(agent_error("Staging file cleanup", e));
    }

    // Delete from DB
    sqlx::query("DELETE FROM sites WHERE id = $1")
        .bind(staging.id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("destroy", e))?;

    tracing::info!("Staging deleted: {}", staging.domain);
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "staging.delete",
        Some("site"),
        Some(&staging.domain),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "domain": staging.domain })))
}
