use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures::stream::StreamExt;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, require_admin, ApiError};
use crate::routes::sites::ProvisionStep;
use crate::services::activity;
use crate::AppState;

// ──────────────────────────────────────────────────────────────
// Request / Response types
// ──────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct AnalyzeRequest {
    pub path: String,
    pub source: Option<String>, // "cpanel", "plesk", "hestiacp"
}

#[derive(serde::Deserialize)]
pub struct ImportRequest {
    pub sites: Option<Vec<ImportSiteItem>>,
    pub databases: Option<Vec<ImportDbItem>>,
}

#[derive(serde::Deserialize, Clone)]
pub struct ImportSiteItem {
    pub domain: String,
    pub doc_root: String,
    pub runtime: String,
}

#[derive(serde::Deserialize, Clone)]
pub struct ImportDbItem {
    pub name: String,
    pub file: String,
    pub engine: String,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Migration {
    pub id: Uuid,
    pub user_id: Uuid,
    pub server_id: Option<Uuid>,
    pub source: String,
    pub status: String,
    pub backup_path: String,
    pub inventory: Option<serde_json::Value>,
    pub selected_items: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

// ──────────────────────────────────────────────────────────────
// Helper: reuse the same emit_step from sites.rs provision logs
// ──────────────────────────────────────────────────────────────

fn emit_step(
    logs: &std::sync::Arc<std::sync::Mutex<std::collections::HashMap<Uuid, (Vec<ProvisionStep>, broadcast::Sender<ProvisionStep>, Instant)>>>,
    id: Uuid,
    step: &str,
    label: &str,
    status: &str,
    message: Option<String>,
) {
    let ev = ProvisionStep {
        step: step.into(),
        label: label.into(),
        status: status.into(),
        message,
    };
    if let Ok(mut map) = logs.lock() {
        if let Some((history, tx, _)) = map.get_mut(&id) {
            history.push(ev.clone());
            let _ = tx.send(ev);
        }
    }
}

// ──────────────────────────────────────────────────────────────
// POST /api/migration/analyze
// ──────────────────────────────────────────────────────────────

/// Panel-side budget for the agent's analyze call.
///
/// Deliberately LONGER than the agent's own extraction budget
/// (`EXTRACT_TIMEOUT_SECS` in the agent's `services::migration`) so that when a
/// giant archive runs out of time it is the *agent* that says so — naming the
/// archive and the budget — instead of this side winning the race and reporting
/// a contentless "agent request timed out". A caller timeout shorter than the
/// callee's does not bound anything the callee did not already bound; it only
/// replaces a real diagnosis with a generic one.
const ANALYZE_AGENT_TIMEOUT_SECS: u64 = 1860;

/// Analyze a panel backup (cPanel/Plesk/HestiaCP).
///
/// **Accepted, not performed.** Walking a cPanel archive is minutes of work for
/// any real account, and every gateway between the browser and this handler
/// gives up long before that: Cloudflare at 100s, the nginx `setup.sh` writes at
/// `proxy_read_timeout 300s`, and this panel's own `TimeoutLayer` at 300s — all
/// of them shorter than the agent call below. Running the analysis inline
/// therefore *guaranteed* a 524/504 for precisely the archives the wizard exists
/// to import (#91), and raising any one of those numbers would only move the
/// size at which it breaks.
///
/// So the row is inserted `analyzing`, the agent call moves to a spawned task,
/// and this returns `202` immediately. The verdict lands in the row — `analyzed`
/// with an inventory, or `failed` carrying the agent's real message — which is
/// what `GET /api/migration/{id}` serves. That row is the durable part: it
/// outlives the request, the tab, and a reload. It does not outlive the process,
/// which is what `finalize_analyzing_on_startup` below exists to close out.
pub async fn analyze(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
    Json(body): Json<AnalyzeRequest>,
) -> Result<(StatusCode, Json<Migration>), ApiError> {
    require_admin(&claims.role)?;
    let path = body.path.trim();
    if path.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Backup path is required"));
    }

    let source = body.source.as_deref().unwrap_or("auto");
    if !["auto", "cpanel", "plesk", "hestiacp"].contains(&source) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Source must be auto, cpanel, plesk, or hestiacp",
        ));
    }

    // Create migration record (status = analyzing)
    let migration: Migration = sqlx::query_as(
        "INSERT INTO migrations (user_id, server_id, source, status, backup_path) \
         VALUES ($1, $2, $3, 'analyzing', $4) RETURNING *",
    )
    .bind(claims.sub)
    .bind(server_id)
    .bind(source)
    .bind(path)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("analyze", e))?;

    let agent_body = serde_json::json!({
        "path": path,
        "source": source,
    });

    let db = state.db.clone();
    let migration_id = migration.id;
    let user_id = claims.sub;
    let email = claims.email.clone();
    let path_owned = path.to_string();
    let source_owned = source.to_string();

    tokio::spawn(async move {
        match agent
            .post_long(
                "/migration/analyze",
                Some(agent_body),
                ANALYZE_AGENT_TIMEOUT_SECS,
            )
            .await
        {
            Ok(inventory) => {
                if let Err(e) = sqlx::query(
                    "UPDATE migrations SET status = 'analyzed', inventory = $1, updated_at = NOW() \
                     WHERE id = $2",
                )
                .bind(&inventory)
                .bind(migration_id)
                .execute(&db)
                .await
                {
                    // The work is done and the panel cannot say so. Leaving the
                    // row 'analyzing' would poll forever, so record the failure
                    // that the operator can actually act on.
                    tracing::error!("Migration {migration_id}: inventory write failed: {e}");
                    let _ = sqlx::query(
                        "UPDATE migrations SET status = 'failed', result = $1, updated_at = NOW() \
                         WHERE id = $2",
                    )
                    .bind(serde_json::json!({
                        "error": format!("Analysis finished but its result could not be saved: {e}")
                    }))
                    .bind(migration_id)
                    .execute(&db)
                    .await;
                    return;
                }

                tracing::info!(
                    "Migration analyzed: {} (source: {})",
                    path_owned,
                    source_owned
                );
                activity::log_activity(
                    &db,
                    user_id,
                    &email,
                    "migration.analyze",
                    Some("migration"),
                    Some(&path_owned),
                    Some(&source_owned),
                    None,
                )
                .await;
            }
            Err(e) => {
                // There is no HTTP response left to shape this into, so the
                // agent's own sentence goes into the row verbatim and the poller
                // reads it out of `result.error`. This is now the ONLY place the
                // reason for a failed analysis is ever recorded — #90 was that
                // reason being replaced by a generic one on the way out.
                tracing::error!("Migration {migration_id}: analysis failed: {e}");
                let _ = sqlx::query(
                    "UPDATE migrations SET status = 'failed', result = $1, updated_at = NOW() \
                     WHERE id = $2",
                )
                .bind(serde_json::json!({ "error": e.to_string() }))
                .bind(migration_id)
                .execute(&db)
                .await;
            }
        }
    });

    Ok((StatusCode::ACCEPTED, Json(migration)))
}

/// Close out analyses that were in flight when the process died.
///
/// The analysis runs in a `tokio::spawn`, so a restart takes it with no chance
/// to write its own verdict — and an `analyzing` row with nothing left to
/// finish it is a spinner the operator can never clear. Same shape, and the same
/// remedy, as `panel_update::finalize_pending_on_startup`: on boot, nothing is
/// running, so every row still claiming to be is stale by definition.
///
/// `importing` rows are swept too, for the same reason and by the same argument
/// — that path has always spawned, so it has always had this hole.
pub async fn finalize_analyzing_on_startup(pool: &sqlx::PgPool) {
    // Two statuses, two different truths, so two sentences. Analysis only reads
    // the archive, so an interrupted one leaves nothing behind and starting again
    // is the whole remedy. An import MUTATES as it goes — it writes nginx vhosts,
    // `sites` rows and database containers one selection at a time — so an
    // interrupted one has done part of the work, and telling the operator to run
    // it again would be telling them to re-import over what already landed.
    // Saying "nothing was left half-done" over that row would be false.
    let sweeps: [(&str, &str); 2] = [
        (
            "analyzing",
            "The panel restarted while this archive was being analysed. Nothing was \
             changed on the server — analysis only reads — so run it again.",
        ),
        (
            "importing",
            "The panel restarted part-way through this import. Whatever had already \
             been imported is still there; the rest was not. Check Sites and Databases \
             for what landed before importing the remainder.",
        ),
    ];

    for (status, message) in sweeps {
        let result = sqlx::query(
            "UPDATE migrations SET status = 'failed', \
             result = COALESCE(result, '{}'::jsonb) || $1::jsonb, updated_at = NOW() \
             WHERE status = $2",
        )
        .bind(serde_json::json!({ "error": message }))
        .bind(status)
        .execute(pool)
        .await;

        match result {
            Ok(r) if r.rows_affected() > 0 => tracing::warn!(
                "Closed out {} migration(s) left '{status}' by a previous process",
                r.rows_affected()
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!("finalize_analyzing_on_startup: '{status}' sweep failed: {e}"),
        }
    }
}

// ──────────────────────────────────────────────────────────────
// GET /api/migration/{id}
// ──────────────────────────────────────────────────────────────

/// Get a single migration record.
pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Migration>, ApiError> {
    require_admin(&claims.role)?;
    let migration: Migration = sqlx::query_as(
        "SELECT * FROM migrations WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("get_one migration", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Migration not found"))?;

    Ok(Json(migration))
}

// ──────────────────────────────────────────────────────────────
// GET /api/migration
// ──────────────────────────────────────────────────────────────

/// List all migrations for the current user.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<Migration>>, ApiError> {
    require_admin(&claims.role)?;
    let migrations: Vec<Migration> = sqlx::query_as(
        "SELECT * FROM migrations WHERE user_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list migration", e))?;

    Ok(Json(migrations))
}

// ──────────────────────────────────────────────────────────────
// POST /api/migration/{id}/import
// ──────────────────────────────────────────────────────────────

/// Start importing selected sites and databases from an analyzed migration.
///
/// **The destination is the row's, not the picker's.** Everything below is a
/// write against one specific machine: it publishes an nginx vhost for a live
/// customer domain, claims that domain, inserts a `sites` row that names its host
/// permanently, and creates database containers. The archive those writes are fed
/// from was unpacked and staged by the analyse step, on the host that step ran
/// against — which is why `analyze` records the server on the migration row in the
/// first place. Reading the destination back off the caller's server selection
/// therefore asked a question the row had already answered, and answered it
/// differently the moment an operator switched servers between analysing and
/// importing: vhosts for somebody's live domains published on a machine that must
/// not serve them, the domain claim registered against that machine, `sites` rows
/// bound to it for good, database containers built there — and then not one file
/// copied, because the staged archive was still sitting on the other host. Half a
/// migration on the wrong box, and the half that landed is the half that answers
/// DNS.
pub async fn import(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ImportRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;
    // Verify migration exists, belongs to user, and is analyzed
    let migration: Migration = sqlx::query_as(
        "SELECT * FROM migrations WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("import", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Migration not found"))?;

    if migration.status != "analyzed" {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("Migration is '{}', expected 'analyzed'", migration.status),
        ));
    }

    // Both halves of the destination come from the row: the id, because it is
    // written into the `sites` rows and passed to the domain claim, and the handle,
    // because it is what actually creates the vhosts and the database containers.
    // Resolved BEFORE the status moves to 'importing' — a migration refused here has
    // not started, and marking it started would leave the operator a row they have
    // to reason about for a run that never happened.
    //
    // A row that names no server is refused rather than guessed at. The column is
    // filled by `analyze` and has been for every migration since the fleet work, so
    // the `None` arm means a row older than that — and the one thing that must never
    // happen to an older row is having a host chosen for it by whoever opened the
    // page. `agent_for_site_server` refuses an unreachable host on the same
    // principle: not importing is recoverable, importing onto the wrong machine is
    // not.
    let server_id = migration.server_id.ok_or_else(|| {
        tracing::warn!("Refusing to import migration {id}: its row names no server");
        err(
            StatusCode::CONFLICT,
            "This migration is not associated with a server",
        )
    })?;
    let agent =
        crate::helpers::agent_for_site_server(&state, Some(server_id), &migration.backup_path)
            .await?;

    let sites = body.sites.unwrap_or_default();
    let databases = body.databases.unwrap_or_default();

    if sites.is_empty() && databases.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Select at least one site or database to import",
        ));
    }

    // Store selected items and update status
    let selected = serde_json::json!({
        "sites": sites.iter().map(|s| &s.domain).collect::<Vec<_>>(),
        "databases": databases.iter().map(|d| &d.name).collect::<Vec<_>>(),
    });

    sqlx::query(
        "UPDATE migrations SET status = 'importing', selected_items = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&selected)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("import", e))?;

    // Extract the agent-side migration_id from the inventory (needed for agent import calls)
    let agent_migration_id = migration
        .inventory
        .as_ref()
        .and_then(|inv| inv.get("id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "Migration inventory missing agent ID"))?;

    // Registered only once nothing above can still bail. Creating the log first
    // meant this `?` returned a 500 while leaving a log and an owner behind for
    // the whole TTL — and `progress` would then answer 200 on a stream nothing
    // would ever emit to, so the page sat on "Provisioning…" until the sweep.
    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        id,
        claims.sub,
        64,
    );

    // Clone everything needed for the spawned task
    let logs = state.provision_logs.clone();
    let db = state.db.clone();
    let jwt_secret = state.config.jwt_secret.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();
    let agent_migration_id = agent_migration_id.clone();
    let migration_source = migration.source.clone();
    // The imported domains are client-supplied (`ImportSiteItem.domain`) and are
    // never reconciled against what the analyse step actually found, so they get
    // the same guard as any other claim. Carried into the task because
    // `is_reserved_domain_for` reads the Host header — BASE_URL is empty on a
    // routine install, which is the whole reason that variant exists.
    let claim_headers = headers.clone();
    // Carried for the same reason as the headers: the claim happens inside the
    // spawned task, after `claims` has been moved. `import` is already
    // admin-only (`require_admin` above), so this can never be a client today —
    // it is passed so that the choke point sees a real role from every caller
    // rather than a placeholder some later refactor would have to remember.
    let claim_role = claims.role.clone();
    // The claim asks every online member whether a Docker app already holds the
    // domain — an app's domain lives only in a container label, so no SQL leg can
    // see it. That needs the registry rather than this task's single handle.
    let claim_agents = state.agents.clone();

    tokio::spawn(async move {
        let mut results = serde_json::json!({
            "sites_imported": [],
            "sites_skipped": [],
            "databases_imported": [],
            "databases_failed": [],
            "errors": [],
        });

        let total_steps = sites.len() + databases.len();
        let mut completed = 0usize;

        // ── Import sites ──────────────────────────────────────
        for site_item in &sites {
            let domain = &site_item.domain;
            let step_key = format!("site_{domain}");

            emit_step(
                &logs,
                id,
                &step_key,
                &format!("Importing site {domain}"),
                "in_progress",
                None,
            );

            // Is this domain claimable? Same guard as every other path — which
            // here also supplies the format and reserved-domain checks the import
            // never had, and extends "already exists" past the `sites` table to
            // git deploys and Docker apps. Note the ORDER below: the vhost is
            // written before the row is inserted, so a domain that slips past
            // this is already on disk by the time the INSERT could reject it.
            let claimed = crate::services::domain_claim::ensure_claimable(
                &db,
                &claim_agents,
                domain,
                &claim_headers,
                crate::services::domain_claim::Holder::New,
                &claim_role,
            )
            .await;

            let domain = &match claimed {
                Ok(d) => d,
                Err((_, reason)) => {
                    let detail = reason
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("domain not available")
                        .to_string();
                    emit_step(
                        &logs,
                        id,
                        &step_key,
                        &format!("Site {domain}"),
                        "done",
                        Some(format!("Skipped ({detail})")),
                    );
                    if let Some(arr) = results["sites_skipped"].as_array_mut() {
                        arr.push(serde_json::json!(domain));
                    }
                    completed += 1;
                    continue;
                }
            };

            // 1. Create nginx site via agent
            let runtime = &site_item.runtime;
            let nginx_body = serde_json::json!({
                "runtime": runtime,
            });

            let agent_path = format!("/nginx/sites/{domain}");
            if let Err(e) = agent.put(&agent_path, nginx_body).await {
                let msg = format!("Nginx config failed for {domain}: {e}");
                tracing::error!("{msg}");
                emit_step(&logs, id, &step_key, &format!("Site {domain}"), "error", Some(msg.clone()));
                if let Some(arr) = results["errors"].as_array_mut() { arr.push(serde_json::json!(msg)); }
                completed += 1;
                continue;
            }

            // 2. Insert site record into DB
            //
            // `server_id` is the migration row's, so the vhost written above, the
            // claim recorded above it, this permanent binding and the file copy
            // below all name one machine. A site row outlives the request that made
            // it and is what every later handler reads to decide which host to talk
            // to, so a wrong value here is not a failed import — it is a site that
            // is quietly asked about on the wrong machine for the rest of its life.
            let site_result = sqlx::query_as::<_, (Uuid,)>(
                "INSERT INTO sites (user_id, server_id, domain, runtime, status) \
                 VALUES ($1, $2, $3, $4, 'active') RETURNING id",
            )
            .bind(user_id)
            .bind(server_id)
            .bind(domain)
            .bind(runtime)
            .fetch_one(&db)
            .await;

            let site_id = match site_result {
                Ok((sid,)) => sid,
                Err(e) => {
                    let msg = format!("DB insert failed for {domain}: {e}");
                    tracing::error!("{msg}");
                    emit_step(&logs, id, &step_key, &format!("Site {domain}"), "error", Some(msg.clone()));
                    if let Some(arr) = results["errors"].as_array_mut() { arr.push(serde_json::json!(msg)); }
                    completed += 1;
                    continue;
                }
            };

            // 3. Copy files via agent
            let import_body = serde_json::json!({
                "migration_id": agent_migration_id,
                "domain": domain,
                "source_dir": site_item.doc_root,
                // Same runtime the vhost was just created with, so the files
                // land in the directory that vhost serves.
                "runtime": runtime,
            });

            if let Err(e) = agent
                .post_long("/migration/import-site", Some(import_body), 300)
                .await
            {
                let msg = format!("File import failed for {domain}: {e}");
                tracing::error!("{msg}");
                emit_step(&logs, id, &step_key, &format!("Site {domain}"), "error", Some(msg.clone()));
                if let Some(arr) = results["errors"].as_array_mut() { arr.push(serde_json::json!(msg)); }
                // Site record exists but files failed — mark as error status
                let _ = sqlx::query("UPDATE sites SET status = 'error', updated_at = NOW() WHERE id = $1")
                    .bind(site_id)
                    .execute(&db)
                    .await;
                completed += 1;
                continue;
            }

            emit_step(
                &logs,
                id,
                &step_key,
                &format!("Site {domain}"),
                "done",
                Some("Imported".into()),
            );
            if let Some(arr) = results["sites_imported"].as_array_mut() {
                arr.push(serde_json::json!({ "domain": domain, "site_id": site_id }));
            }

            completed += 1;
            tracing::info!(
                "Migration {id}: imported site {domain} ({completed}/{total_steps})"
            );
        }

        // ── Import databases ──────────────────────────────────
        for db_item in &databases {
            let db_name = &db_item.name;
            let engine = &db_item.engine;
            let step_key = format!("db_{db_name}");

            emit_step(
                &logs,
                id,
                &step_key,
                &format!("Importing database {db_name}"),
                "in_progress",
                None,
            );

            // Generate password
            let password = Uuid::new_v4().to_string().replace('-', "");

            // Find available port
            let (range_start, range_end) = match engine.as_str() {
                "mysql" | "mariadb" => (3307, 3400),
                _ => (5433, 5500),
            };

            let port_row: Option<(i32,)> = sqlx::query_as(
                "SELECT s.port FROM generate_series($1::int, $2::int) AS s(port) \
                 WHERE s.port NOT IN (SELECT port FROM databases WHERE port IS NOT NULL) \
                 LIMIT 1",
            )
            .bind(range_start)
            .bind(range_end)
            .fetch_optional(&db)
            .await
            .unwrap_or(None);

            let port = match port_row {
                Some((p,)) => p,
                None => {
                    let msg = format!("No available port for database {db_name}");
                    tracing::error!("{msg}");
                    emit_step(&logs, id, &step_key, &format!("Database {db_name}"), "error", Some(msg.clone()));
                    if let Some(arr) = results["databases_failed"].as_array_mut() { arr.push(serde_json::json!(db_name)); }
                    if let Some(arr) = results["errors"].as_array_mut() { arr.push(serde_json::json!(msg)); }
                    completed += 1;
                    continue;
                }
            };

            // Create database container via agent
            let create_body = serde_json::json!({
                "name": db_name,
                "engine": engine,
                "password": password,
                "port": port,
            });

            let (container_id, container_name) = match agent.post("/databases", Some(create_body)).await {
                Ok(resp) => {
                    let cid = resp.get("container_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let cname = resp.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    (cid, cname)
                }
                Err(e) => {
                    let msg = format!("Database container creation failed for {db_name}: {e}");
                    tracing::error!("{msg}");
                    emit_step(&logs, id, &step_key, &format!("Database {db_name}"), "error", Some(msg.clone()));
                    if let Some(arr) = results["databases_failed"].as_array_mut() { arr.push(serde_json::json!(db_name)); }
                    if let Some(arr) = results["errors"].as_array_mut() { arr.push(serde_json::json!(msg)); }
                    completed += 1;
                    continue;
                }
            };

            // Insert DB record (not linked to a specific site — migration imports are standalone)
            let encrypted_password = crate::services::secrets_crypto::encrypt_credential(&password, &jwt_secret)
                .unwrap_or_else(|_| password.clone());
            let _ = sqlx::query(
                "INSERT INTO databases (engine, name, db_user, db_password_enc, container_id, port) \
                 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
            )
            .bind(engine)
            .bind(db_name)
            .bind(db_name)
            .bind(&encrypted_password)
            .bind(&container_id)
            .bind(port)
            .execute(&db)
            .await;

            // Wait for engine to be ready
            emit_step(
                &logs,
                id,
                &step_key,
                &format!("Waiting for {db_name} engine"),
                "in_progress",
                None,
            );
            tokio::time::sleep(Duration::from_secs(5)).await;

            // Import SQL dump via agent
            let import_body = serde_json::json!({
                "migration_id": agent_migration_id,
                "sql_file": db_item.file,
                "container_name": container_name,
                "db_name": db_name,
                "engine": engine,
                "user": db_name,
                "password": password,
            });

            match agent
                .post_long("/migration/import-database", Some(import_body), 600)
                .await
            {
                Ok(_) => {
                    emit_step(
                        &logs,
                        id,
                        &step_key,
                        &format!("Database {db_name}"),
                        "done",
                        Some("Imported".into()),
                    );
                    if let Some(arr) = results["databases_imported"].as_array_mut() {
                        arr.push(serde_json::json!({ "name": db_name, "port": port }));
                    }
                }
                Err(e) => {
                    let msg = format!("SQL import failed for {db_name}: {e}");
                    tracing::error!("{msg}");
                    emit_step(&logs, id, &step_key, &format!("Database {db_name}"), "error", Some(msg.clone()));
                    if let Some(arr) = results["databases_failed"].as_array_mut() { arr.push(serde_json::json!(db_name)); }
                    if let Some(arr) = results["errors"].as_array_mut() { arr.push(serde_json::json!(msg)); }
                }
            }

            completed += 1;
            tracing::info!(
                "Migration {id}: processed database {db_name} ({completed}/{total_steps})"
            );
        }

        // ── Finalize ──────────────────────────────────────────
        let has_errors = !results["errors"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true);
        let final_status = if has_errors { "done" } else { "done" }; // always "done" — errors are in result JSON

        let _ = sqlx::query(
            "UPDATE migrations SET status = $1, result = $2, updated_at = NOW() WHERE id = $3",
        )
        .bind(final_status)
        .bind(&results)
        .bind(id)
        .execute(&db)
        .await;

        activity::log_activity(
            &db,
            user_id,
            &email,
            "migration.import",
            Some("migration"),
            Some(&migration_source),
            None,
            None,
        )
        .await;

        emit_step(&logs, id, "complete", "Migration complete", "done", None);
        tracing::info!("Migration {id} completed");

        // Keep the log channel alive for 60s so the frontend can catch up
        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "migration_id": id })),
    ))
}

// ──────────────────────────────────────────────────────────────
// GET /api/migration/{id}/progress — SSE stream
// ──────────────────────────────────────────────────────────────

/// SSE stream of migration import progress.
pub async fn progress(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, axum::BoxError>>>, ApiError> {
    require_admin(&claims.role)?;
    // Verify ownership
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM migrations WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("progress", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Migration not found"));
    }

    // As in sites::provision_log, the row check above is kept alongside the
    // per-key owner check.
    let (snapshot, rx) = crate::helpers::open_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        id,
        claims.sub,
        "No active import for this migration",
    )?;

    // First yield snapshot events, then stream live updates
    let snapshot_stream = futures::stream::iter(snapshot.into_iter().map(|step| {
        let data = serde_json::to_string(&step).unwrap_or_default();
        Ok(Event::default().data(data))
    }));

    let live_stream = BroadcastStream::new(rx).filter_map(|result| async {
        match result {
            Ok(step) => {
                let data = serde_json::to_string(&step).ok()?;
                Some(Ok(Event::default().data(data)))
            }
            Err(_) => None,
        }
    });

    let stream = snapshot_stream.chain(live_stream);

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

// ──────────────────────────────────────────────────────────────
// DELETE /api/migration/{id}
// ──────────────────────────────────────────────────────────────

/// Delete a migration record and clean up temp files.
///
/// The staging directory this cleans up is on the host that unpacked the archive,
/// which the row names — so that is where the cleanup goes, rather than wherever
/// the server picker happens to be pointing. Sent to any other machine the call
/// succeeds and does nothing: no staging directory exists there under this id, so
/// the agent has nothing to delete and says so cheerfully, while the real
/// extracted copy — a whole cPanel account's files — stays on disk with the panel
/// record that could have found it now deleted.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let migration: Migration = sqlx::query_as(
        "SELECT * FROM migrations WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("remove migration", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Migration not found"))?;

    // Ask agent to clean up extracted temp files (best-effort)
    let agent_migration_id = migration
        .inventory
        .as_ref()
        .and_then(|inv| inv.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let cleanup_body = serde_json::json!({
        "migration_id": agent_migration_id,
    });

    // An unresolvable host is not fatal here, and that is the one place this
    // differs from the import. The record is panel state: it has to stay deletable
    // after the machine that staged it has been decommissioned, or a fleet
    // shrinking by one server leaves rows nobody can ever clear. So the cleanup is
    // skipped and said out loud — the files are named as still being there — rather
    // than aimed at a host that never had them, which is the outcome that looks
    // like success and is not one.
    match crate::helpers::agent_for_site_server(&state, migration.server_id, &migration.backup_path)
        .await
    {
        Ok(agent) => {
            if let Err(e) = agent.post("/migration/cleanup", Some(cleanup_body)).await {
                tracing::warn!("Migration cleanup agent call failed for {id}: {e}");
            }
        }
        Err(_) => tracing::warn!(
            "Migration {id}: deleting the record without cleaning up — the server it was \
             staged on could not be resolved, so the extracted files remain on that host"
        ),
    }

    // Delete from DB
    sqlx::query("DELETE FROM migrations WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove migration", e))?;

    // Remove any lingering provision log channel
    crate::helpers::forget_provision_log(&state.provision_logs, &state.deploy_owners, id);

    tracing::info!("Migration deleted: {id}");
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "migration.delete",
        Some("migration"),
        None,
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}
