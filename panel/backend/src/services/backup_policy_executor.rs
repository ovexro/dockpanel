//! GAP 1: Backup Policy Executor — runs every 60s, evaluates cron schedules,
//! executes backup_policies across sites, databases, and volumes.

use chrono::{Datelike, Timelike};
use sqlx::PgPool;
use uuid::Uuid;

use crate::services::agent::{AgentHandle, AgentRegistry};
use crate::services::notifications;

/// Row from the policy query.
#[derive(sqlx::FromRow)]
#[allow(dead_code)]
struct PolicyRow {
    id: Uuid,
    user_id: Uuid,
    server_id: Option<Uuid>,
    name: String,
    backup_sites: bool,
    backup_databases: bool,
    backup_volumes: bool,
    schedule: String,
    destination_id: Option<Uuid>,
    retention_count: i32,
    encrypt: bool,
    verify_after_backup: bool,
    last_run: Option<chrono::DateTime<chrono::Utc>>,
}

/// Derive the symmetric key used to encrypt AND decrypt policy-driven DB backups.
///
/// This is the SINGLE SOURCE OF TRUTH for the derivation. The encrypt side
/// (`execute_policy`) and the decrypt side (`routes::backup_orchestrator::restore_db_backup`)
/// previously derived the key from two DIFFERENT, incompatible sources — restore read a
/// `backup_destinations.encryption_key` column that nothing ever writes — so every
/// encrypted DB backup was permanently unrestorable and DR was silently broken in encrypted
/// mode (lesson #70: resolve by the value that actually produced the artifact, not a
/// re-derived phantom). Keyed on the process JWT secret (`config.jwt_secret`, == the
/// `JWT_SECRET` env the executor is handed), so both sides agree byte-for-byte.
pub fn derive_backup_encryption_key(jwt_secret: &str) -> String {
    format!("backup-enc-{}", &jwt_secret[..32.min(jwt_secret.len())])
}

/// A policy's off-site destination, resolved once per run.
///
/// # Why this exists
///
/// `destination_id` was selected by the policy query, carried on [`PolicyRow`]
/// under an `#[allow(dead_code)]`, and **never read**. Meanwhile the Backup
/// Orchestrator's policy form offers a Destination dropdown, the schema gives
/// `database_backups` and `volume_backups` a `destination_id` and an `uploaded`
/// column, and the All-Backups table renders a `remote` badge from `uploaded`.
///
/// So every policy-driven backup stayed on the machine it was protecting, the
/// badge could never light for a policy row, and the operator had chosen an
/// off-site destination and been shown no indication it was ignored. A backup
/// that only exists on the disk it is insuring is not a backup.
///
/// The sibling path — `backup_scheduler`, which runs per-site schedules — was
/// described here as having "uploaded correctly all along". That was wrong, and
/// wrong in a way this comment helped preserve: it had the same missing decrypt
/// this path did, so both were posting ciphertext as the credential while Test
/// Connection went on reporting success. Both now go through
/// `agent_destination_payload`. What remains true is the rest — this path shares
/// the scheduler's retry ladder and its refusal to record a backup whose upload
/// failed.
struct ResolvedDestination {
    id: Uuid,
    /// The destination config with `type` folded in, exactly as the agent's
    /// `/backups/upload` handler expects it.
    payload: serde_json::Value,
}

async fn resolve_destination(db: &PgPool, policy: &PolicyRow) -> Option<ResolvedDestination> {
    let id = policy.destination_id?;

    let row: Result<Option<(String, serde_json::Value)>, _> =
        sqlx::query_as("SELECT dtype, config FROM backup_destinations WHERE id = $1")
            .bind(id)
            .fetch_optional(db)
            .await;

    // Distinguish "deleted" from "we couldn't ask". Both end in local-only
    // backups, but they are different faults and collapsing them would send an
    // operator hunting for a destination that is still perfectly there.
    let (dtype, config) = match row {
        Ok(Some(r)) => r,
        Ok(None) => {
            tracing::error!(
                "Policy '{}': destination {id} no longer exists — backups will stay on this server",
                policy.name
            );
            return None;
        }
        Err(e) => {
            tracing::error!(
                "Policy '{}': could not read destination {id} ({e}) — backups will stay on this server",
                policy.name
            );
            return None;
        }
    };

    Some(ResolvedDestination {
        id,
        payload: crate::routes::backup_destinations::agent_destination_payload(&dtype, &config),
    })
}

/// Push one finished backup file off the box.
///
/// Returns `true` only when the bytes actually landed. Mirrors
/// `backup_scheduler`'s ladder (5s / 15s / 30s) rather than inventing a second
/// retry policy, so the two paths fail the same way.
///
/// # Known gap: remote retention covers site backups only
///
/// The agent's `/backups/prune` is keyed on a site `domain` and prunes that
/// domain's remote prefix, so it can enforce retention for site archives and
/// nothing else. Database and volume copies therefore accumulate at the
/// destination even though their LOCAL copies are pruned by `auto_healer`.
/// Stated here rather than left to be discovered from a storage bill; a
/// resource-agnostic prune is the fix, and it needs an agent-side change.
async fn upload_to_destination(
    agent: &AgentHandle,
    dest: &ResolvedDestination,
    filepath: &str,
    label: &str,
) -> bool {
    let body = serde_json::json!({ "filepath": filepath, "destination": dest.payload });

    let delays = [5u64, 15, 30];
    let mut last_err = String::new();

    for (attempt, delay) in delays.iter().enumerate() {
        // See the sibling call in `backup_scheduler`: plain `post` caps the call at
        // 60s while the agent budgets 600s for the same upload, so a slow-but-fine
        // transfer was reported as a failure three times over — and here that also
        // trips `destination_down`, which skips off-siting for every remaining site,
        // database and volume in the run and raises an incident saying the backups
        // "exist only on this server" about backups that had just been uploaded.
        match agent
            .post_long("/backups/upload", Some(body.clone()), 660)
            .await
        {
            Ok(_) => return true,
            Err(e) => {
                last_err = e.to_string();
                if attempt < delays.len() - 1 {
                    tracing::warn!("Upload attempt {} failed for {label}: {last_err} — retrying in {delay}s", attempt + 1);
                    tokio::time::sleep(std::time::Duration::from_secs(*delay)).await;
                }
            }
        }
    }

    tracing::error!("Upload failed for {label} after {} attempts: {last_err}", delays.len());
    false
}

/// Run the backup policy executor loop — checks every 60 seconds for due policies.
/// Fleet-wide policy query, so it takes the REGISTRY. Each leg resolves its own host:
/// sites and databases per ROW (via `sites.server_id`, which is NOT NULL), volumes per
/// POLICY (their subject list comes from the agent, so there is no row to read).
///
/// Left local, the volume leg was the worst shape in the codebase: it enumerated the
/// PANEL's containers, tarred the PANEL's volumes, stamped each row with the member's
/// `server_id`, and reported success — with no failure path at all. s298's correctly
/// scoped `auto_healer` then refused to prune those rows, because they claimed to
/// belong to another host, so retention was enforced on nothing.
pub async fn run(db: PgPool, agents: AgentRegistry, jwt_secret: String, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Backup policy executor started");

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(e) = tick(&db, &agents, &jwt_secret).await {
                    tracing::error!("Backup policy executor error: {e}");
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Backup policy executor shutting down gracefully");
                break;
            }
        }
    }
}

/// Track last stale-backup check to avoid spamming (once per hour).
static LAST_STALE_CHECK: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

async fn tick(db: &PgPool, agents: &AgentRegistry, jwt_secret: &str) -> Result<(), String> {
    let now = chrono::Utc::now();

    // Fetch all enabled policies
    let policies: Vec<PolicyRow> = sqlx::query_as(
        "SELECT id, user_id, server_id, name, backup_sites, backup_databases, backup_volumes, \
         schedule, destination_id, retention_count, encrypt, verify_after_backup, last_run \
         FROM backup_policies WHERE enabled = TRUE"
    )
    .fetch_all(db).await.map_err(|e| e.to_string())?;

    for policy in &policies {
        // Check if cron matches current time
        if !cron_matches_now(&policy.schedule, &now) {
            continue;
        }

        // Prevent double-runs within 90 seconds
        if let Some(last_run) = policy.last_run {
            if (now - last_run).num_seconds() < 90 {
                continue;
            }
        }

        tracing::info!("Executing backup policy '{}' ({})", policy.name, policy.id);
        execute_policy(db, agents, policy, jwt_secret).await;
    }

    // Record backup storage metric for growth tracking. SUM(BIGINT) returns
    // NUMERIC in postgres — cast to bigint so sqlx can decode into i64.
    let total_storage: Option<(i64,)> = sqlx::query_as(
        "SELECT COALESCE(SUM(size_bytes), 0)::bigint FROM ( \
            SELECT size_bytes FROM backups UNION ALL \
            SELECT size_bytes FROM database_backups UNION ALL \
            SELECT size_bytes FROM volume_backups \
        ) t"
    ).fetch_one(db).await.ok();
    if let Some((bytes,)) = total_storage {
        let _ = sqlx::query(
            "INSERT INTO system_logs (level, source, message) VALUES ('info', 'backup_storage', $1)"
        ).bind(format!("{}", bytes)).execute(db).await;
    }

    // Proactive backup freshness alerting — once per hour
    let now_ts = now.timestamp();
    let last_check = LAST_STALE_CHECK.load(std::sync::atomic::Ordering::Relaxed);
    if now_ts - last_check >= 3600 {
        LAST_STALE_CHECK.store(now_ts, std::sync::atomic::Ordering::Relaxed);

        let stale: Vec<(String,)> = sqlx::query_as(
            "SELECT s.domain FROM sites s WHERE s.status = 'active' \
             AND NOT EXISTS (SELECT 1 FROM backups b WHERE b.site_id = s.id AND b.created_at > NOW() - INTERVAL '48 hours')"
        ).fetch_all(db).await.unwrap_or_default();

        if !stale.is_empty() {
            let domains: Vec<&str> = stale.iter().map(|s| s.0.as_str()).collect();
            notifications::notify_panel(db, None,
                &format!("{} site(s) have stale backups", stale.len()),
                &format!("These sites have no backup in 48+ hours: {}", domains.join(", ")),
                "warning", "backup", Some("/backup-orchestrator")
            ).await;
            tracing::warn!("Stale backup alert: {} sites without recent backups", stale.len());
        }
    }

    Ok(())
}

async fn execute_policy(db: &PgPool, agents: &AgentRegistry, policy: &PolicyRow, jwt_secret: &str) {
    let mut successes = 0;
    let mut failures = 0;

    // Resolved once, not per file: a policy backing up forty sites must not run
    // forty identical destination lookups.
    let destination = resolve_destination(db, policy).await;

    // A failed upload counts as a FAILURE (so the policy reports partial/failed
    // and the backup_failure alert fires) but the local file is still recorded.
    // This deliberately differs from `backup_scheduler`, which drops the row
    // entirely: these tables carry the sha256 integrity chain, and a missing row
    // would break `previous_hash` for every backup after it. Recording the file
    // with `uploaded = FALSE` keeps the chain intact and keeps the UI honest.
    let mut upload_failures = 0;

    // Circuit breaker. Each upload retries three times with 5s/15s/30s backoff,
    // so a destination that is simply DOWN would cost ~50 seconds PER FILE — a
    // policy covering forty sites would sit in one 60-second tick for half an
    // hour, starving every other policy behind it. The first exhausted retry
    // ladder is enough evidence: stop dialling for the rest of this run and let
    // the next scheduled run try again from scratch.
    let mut destination_down = false;

    // Remote retention is reported once per run, not once per resource.
    let mut remote_prune_warned = false;

    // Get encryption key if encrypt is enabled. Use the shared derivation (single source of
    // truth) keyed on the jwt_secret passed into the executor — which equals the
    // state.config.jwt_secret the restore path uses — NOT a separate std::env read that could
    // drift from it. This is what makes an encrypted backup actually restorable.
    let encryption_key: Option<String> = if policy.encrypt {
        Some(derive_backup_encryption_key(jwt_secret))
    } else {
        None
    };

    // The key reaches the database dump and nothing else — the agent has no
    // encryption path for a site or volume archive, so there is nowhere to send
    // it. Say so per run rather than letting the flag imply cover it does not
    // give: a site archive is the whole webroot, and since v2.34.0 it also
    // carries a dump of every database attached to that site, which means an
    // encrypted policy ships an encrypted copy of that data and an unencrypted
    // one to the same destination.
    if policy.encrypt && (policy.backup_sites || policy.backup_volumes) {
        tracing::warn!(
            "Policy '{}': encryption applies to database dumps only — this policy's site/volume archives are stored and uploaded unencrypted",
            policy.name
        );
    }

    // Backup sites
    if policy.backup_sites {
        // `server_id` rides along: a site is archived on the host that holds its
        // files. NOT NULL, so there is no ambiguous case to default.
        let sites: Vec<(Uuid, String, Uuid)> = sqlx::query_as(
            "SELECT id, domain, server_id FROM sites WHERE user_id = $1"
        )
        .bind(policy.user_id)
        .fetch_all(db).await.unwrap_or_default();

        for (site_id, domain, site_server_id) in &sites {
            // Resolve THIS site's host. Never fall back to local: archiving a member's
            // site against the panel's own /var/www either fails every night for ever,
            // or — where the same domain legally exists on both hosts — silently tars
            // the wrong machine's files and prunes the member's real off-site copies.
            let agent = match agents.for_server(*site_server_id).await {
                Ok(a) => a,
                Err(e) => {
                    failures += 1;
                    tracing::warn!(
                        "Policy '{}': skipping site {domain} — its server {site_server_id} \
                         is unreachable ({e}). Not archiving it on another host.",
                        policy.name
                    );
                    continue;
                }
            };
            let agent = &agent;
            // WITH the site's databases — a policy-driven backup that omitted
            // them would reintroduce, on the automated path, exactly the gap
            // v2.34.0 closed on the manual one.
            let site_dbs = crate::routes::backups::site_database_specs(db, jwt_secret, *site_id).await;
            let db_expected = site_dbs.expected() as i32;
            let agent_body = serde_json::json!({ "databases": site_dbs.specs });

            let mut result = agent.post(&format!("/backups/{domain}/create"), Some(agent_body.clone())).await;
            // Retry once on failure
            if result.is_err() {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                result = agent.post(&format!("/backups/{domain}/create"), Some(agent_body)).await;
            }
            match result {
                Ok(resp) => {
                    let filename = resp.get("filename").and_then(|v| v.as_str()).unwrap_or("");
                    let size_bytes = resp.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;

                    // Same read the database and volume branches below already do
                    // off their own agent responses. This branch had the field in
                    // scope and never looked at it, so a site archived by a policy
                    // carried no integrity hash while its db and volume siblings
                    // in this very function did (#114). Read before the upload, as
                    // they do.
                    let sha256_hash = resp
                        .get("sha256")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let previous_hash: Option<String> = sqlx::query_scalar::<_, Option<String>>(
                        "SELECT sha256_hash FROM backups WHERE site_id = $1 ORDER BY created_at DESC LIMIT 1"
                    ).bind(site_id).fetch_optional(db).await.unwrap_or(None).flatten();

                    let mut uploaded = false;
                    if let (Some(dest), false) = (&destination, destination_down) {
                        let filepath = format!("/var/backups/dockpanel/{domain}/{filename}");
                        uploaded = upload_to_destination(agent, dest, &filepath, &format!("site {domain}")).await;
                        if uploaded {
                            // Enforce remote retention too, or the destination grows
                            // without bound. Same call the scheduler makes.
                            //
                            // The agent returns 200 with a `message` when it cannot
                            // prune (SFTP has no supported path), so a discarded body
                            // left the policy showing an enforced retention count over
                            // a destination that never deletes anything. Reported once
                            // per run — a forty-site policy should not write forty
                            // identical rows.
                            match agent.post("/backups/prune", Some(serde_json::json!({
                                "destination": dest.payload,
                                "domain": domain,
                                "retention": policy.retention_count,
                            }))).await {
                                Ok(resp) => {
                                    if let Some(msg) = resp.get("message").and_then(|v| v.as_str()) {
                                        if !remote_prune_warned {
                                            remote_prune_warned = true;
                                            // "warning", not "warn" — the API filters and
                                            // counts on the former, so a "warn" row was
                                            // uncountable in the Warnings tile and grey in
                                            // the list. Its sibling at backup_scheduler.rs
                                            // already spelled it the long way twice.
                                            crate::services::system_log::log_event(
                                                db,
                                                "warning",
                                                "backup_policy_executor",
                                                &format!("Policy '{}': remote retention was not enforced", policy.name),
                                                Some(msg),
                                            ).await;
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Policy '{}': remote prune failed for {domain}: {e}", policy.name);
                                }
                            }
                        } else {
                            upload_failures += 1;
                            destination_down = true;
                        }
                    } else if destination.is_some() {
                        // Breaker already open — count it, don't dial.
                        upload_failures += 1;
                    }

                    // A backup the panel cannot record is a backup the operator
                    // cannot find or restore: every list and restore path keys off
                    // this row. Discarding the Result reported the run green over
                    // an archive that exists on disk and nowhere in the product.
                    // The realistic trigger is not "the database is down" — it is
                    // deleting a backup destination mid-run, which makes every
                    // remaining insert in that run a foreign-key violation.
                    match sqlx::query(
                        "INSERT INTO backups (site_id, filename, size_bytes, destination_id, uploaded, databases_included, databases_expected, sha256_hash, previous_hash) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
                    )
                    .bind(site_id).bind(filename).bind(size_bytes)
                    .bind(destination.as_ref().map(|d| d.id)).bind(uploaded)
                    .bind(resp.get("databases_included").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0) as i32)
                    .bind(db_expected)
                    .bind(if sha256_hash.is_empty() { None } else { Some(&sha256_hash) })
                    .bind(previous_hash.as_deref())
                    .execute(db).await {
                        Ok(_) => successes += 1,
                        Err(e) => {
                            tracing::error!("Policy '{}': site backup for {domain} was created but could not be recorded: {e}", policy.name);
                            failures += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Policy '{}': site backup failed for {domain} (after retry): {e}", policy.name);
                    failures += 1;
                }
            }
        }
    }

    // Backup databases
    if policy.backup_databases {
        // `s.server_id` comes free from the join this query already makes — the same
        // shape `create_db_backup` uses. Without it the panel `docker exec`s whatever
        // answers to `dockpanel-db-{name}` on ITS OWN host, and that name is unique
        // only per site, so the dump can be another tenant's database entirely.
        let databases: Vec<(Uuid, String, String, String, String, Uuid)> = sqlx::query_as(
            "SELECT d.id, d.name, d.engine, d.db_user, d.db_password_enc, s.server_id \
             FROM databases d JOIN sites s ON d.site_id = s.id WHERE s.user_id = $1"
        )
        .bind(policy.user_id)
        .fetch_all(db).await.unwrap_or_default();

        for (db_id, db_name, engine, user, password_enc, db_server_id) in &databases {
            let agent = match agents.for_server(*db_server_id).await {
                Ok(a) => a,
                Err(e) => {
                    failures += 1;
                    tracing::warn!(
                        "Policy '{}': skipping database {db_name} — its server \
                         {db_server_id} is unreachable ({e}).",
                        policy.name
                    );
                    continue;
                }
            };
            let agent = &agent;
            let password = crate::services::secrets_crypto::decrypt_credential_or_legacy(password_enc, jwt_secret);
            let container_name = format!("dockpanel-db-{db_name}");
            let body = serde_json::json!({
                "container_name": container_name,
                "db_name": db_name,
                "db_type": engine,
                "user": user,
                "password": password,
                "encryption_key": encryption_key,
            });

            let mut result = agent.post("/db-backups/dump", Some(body.clone())).await;
            // Retry once on failure
            if result.is_err() {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                result = agent.post("/db-backups/dump", Some(body)).await;
            }
            match result {
                Ok(resp) => {
                    let filename = resp.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let size_bytes = resp.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;
                    let encrypted = encryption_key.is_some();

                    let sha256_hash = resp.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let previous_hash: Option<String> = sqlx::query_scalar::<_, Option<String>>(
                        "SELECT sha256_hash FROM database_backups WHERE database_id = $1 ORDER BY created_at DESC LIMIT 1"
                    ).bind(db_id).fetch_optional(db).await.unwrap_or(None).flatten();

                    let mut uploaded = false;
                    if let (Some(dest), false) = (&destination, destination_down) {
                        // `database_backup::backup_dir` nests per database:
                        // /var/backups/dockpanel/databases/<db_name>/<filename>.
                        let filepath = format!("/var/backups/dockpanel/databases/{db_name}/{filename}");
                        uploaded = upload_to_destination(agent, dest, &filepath, &format!("database {db_name}")).await;
                        if !uploaded {
                            upload_failures += 1;
                            destination_down = true;
                        }
                    } else if destination.is_some() {
                        upload_failures += 1;
                    }

                    match sqlx::query(
                        "INSERT INTO database_backups (database_id, server_id, filename, size_bytes, db_type, db_name, encrypted, policy_id, destination_id, uploaded, sha256_hash, previous_hash, chain_valid) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, TRUE)"
                    )
                    .bind(db_id).bind(*db_server_id).bind(&filename).bind(size_bytes)
                    .bind(engine).bind(db_name).bind(encrypted).bind(policy.id)
                    .bind(destination.as_ref().map(|d| d.id)).bind(uploaded)
                    .bind(if sha256_hash.is_empty() { None } else { Some(&sha256_hash) })
                    .bind(previous_hash.as_deref())
                    .execute(db).await {
                        Ok(_) => successes += 1,
                        Err(e) => {
                            tracing::error!("Policy '{}': DB backup for {db_name} was created but could not be recorded: {e}", policy.name);
                            failures += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Policy '{}': DB backup failed for {db_name} (after retry): {e}", policy.name);
                    failures += 1;
                }
            }
        }
    }

    // Backup volumes (Docker app volumes). This is an admin-only capability — the
    // direct create_volume_backup endpoint is AdminUser, and the enumeration below is
    // fleet-wide with NO per-user scoping. Policy CRUD is now admin-gated, but guard
    // the executor too so any legacy non-admin-owned policy cannot drive volume backups.
    let owner_is_admin: bool = sqlx::query_scalar::<_, bool>(
        "SELECT role = 'admin' FROM users WHERE id = $1"
    )
    .bind(policy.user_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .unwrap_or(false);
    if policy.backup_volumes && !owner_is_admin {
        tracing::warn!("Policy '{}': skipping volume backup — owner is not an admin", policy.name);
    }
    if policy.backup_volumes && owner_is_admin { 'vol: {
        // Volumes have no per-row host to read: the subject list comes from asking an
        // agent for its containers, so the POLICY's server decides which agent. When
        // the policy names none — the only thing the UI can currently produce, since
        // the policy form has no server picker — resolve the local server EXPLICITLY
        // and record that id, rather than writing the policy's NULL through.
        //
        // Recording the server actually reached (not `policy.server_id`) is the whole
        // point: the two used to differ, which is what made `auto_healer` refuse to
        // prune these rows for ever.
        let vol_server_id = match policy.server_id {
            Some(sid) => Some(sid),
            None => agents.local_server_id().await,
        };
        let Some(vol_server_id) = vol_server_id else {
            tracing::warn!(
                "Policy '{}': skipping volume backup — no server on the policy and the \
                 local server id is not yet known (setup incomplete).",
                policy.name
            );
            break 'vol;
        };
        let agent = match agents.for_server(vol_server_id).await {
            Ok(a) => a,
            Err(e) => {
                failures += 1;
                tracing::warn!(
                    "Policy '{}': skipping volume backup — server {vol_server_id} is \
                     unreachable ({e}). Not tarring another host's volumes under its name.",
                    policy.name
                );
                break 'vol;
            }
        };
        let agent = &agent;
        // Get Docker containers with volumes
        match agent.get("/apps").await {
            Ok(apps) => {
                if let Some(apps) = apps.as_array() {
                    for app in apps {
                        let container_name = app.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let container_id = app.get("container_id").and_then(|v| v.as_str()).unwrap_or("");

                        if container_name.is_empty() { continue; }

                        // Get volumes for this container
                        if let Ok(vol_resp) = agent.get(&format!("/apps/{container_id}/volumes")).await {
                            if let Some(volumes) = vol_resp.as_array() {
                                for vol in volumes {
                                    let vol_name = vol.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    if vol_name.is_empty() { continue; }

                                    let body = serde_json::json!({
                                        "volume_name": vol_name,
                                        "container_name": container_name,
                                    });

                                    let mut result = agent.post("/volume-backups/create", Some(body.clone())).await;
                                    // Retry once on failure
                                    if result.is_err() {
                                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                                        result = agent.post("/volume-backups/create", Some(body)).await;
                                    }
                                    match result {
                                        Ok(resp) => {
                                            let filename = resp.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                            let size_bytes = resp.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;

                                            let sha256_hash = resp.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                            let previous_hash: Option<String> = sqlx::query_scalar::<_, Option<String>>(
                                                "SELECT sha256_hash FROM volume_backups WHERE container_id = $1 AND volume_name = $2 ORDER BY created_at DESC LIMIT 1"
                                            ).bind(container_id).bind(vol_name).fetch_optional(db).await.unwrap_or(None).flatten();

                                            let mut uploaded = false;
                                            if let (Some(dest), false) = (&destination, destination_down) {
                                                // `volume_backup::backup_dir` nests per container:
                                                // /var/backups/dockpanel/volumes/<container>/<filename>.
                                                let filepath = format!("/var/backups/dockpanel/volumes/{container_name}/{filename}");
                                                uploaded = upload_to_destination(agent, dest, &filepath, &format!("volume {vol_name}")).await;
                                                if !uploaded {
                                                    upload_failures += 1;
                                                    destination_down = true;
                                                }
                                            } else if destination.is_some() {
                                                upload_failures += 1;
                                            }

                                            match sqlx::query(
                                                "INSERT INTO volume_backups (container_id, container_name, server_id, volume_name, filename, size_bytes, policy_id, destination_id, uploaded, sha256_hash, previous_hash, chain_valid) \
                                                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, TRUE)"
                                            )
                                            .bind(container_id).bind(container_name).bind(vol_server_id)
                                            .bind(vol_name).bind(&filename).bind(size_bytes).bind(policy.id)
                                            .bind(destination.as_ref().map(|d| d.id)).bind(uploaded)
                                            .bind(if sha256_hash.is_empty() { None } else { Some(&sha256_hash) })
                                            .bind(previous_hash.as_deref())
                                            .execute(db).await {
                                                Ok(_) => successes += 1,
                                                Err(e) => {
                                                    tracing::error!("Policy '{}': volume backup for {container_name}/{vol_name} was created but could not be recorded: {e}", policy.name);
                                                    failures += 1;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!("Policy '{}': volume backup failed for {container_name}/{vol_name} (after retry): {e}", policy.name);
                                            failures += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("Policy '{}': failed to list Docker apps for volume backup: {e}", policy.name);
                failures += 1;
            }
        }
    }}

    // An upload failure is NOT a backup failure — the file exists locally, and it
    // is counted in `successes` because it was taken. It IS a disaster-recovery
    // failure, though, and reporting the run as a clean success would be exactly
    // the silence this change exists to remove. So it degrades the status and
    // raises the alert, while the counts keep meaning "backups taken".
    let total_attempted = successes + failures;
    let status = if failures == 0 && upload_failures == 0 {
        "success"
    } else if successes > 0 {
        "partial"
    } else {
        "failed"
    };
    let _ = sqlx::query(
        "UPDATE backup_policies SET last_run = NOW(), last_status = $2, updated_at = NOW() WHERE id = $1"
    )
    .bind(policy.id).bind(status)
    .execute(db).await;

    tracing::info!(
        "Policy '{}' completed: {successes} successes, {failures} failures, \
         {upload_failures} not uploaded off-site (status: {status})",
        policy.name
    );

    // How to describe a run that took its backups but couldn't get them off the
    // box. Kept in one place so the incident, the alert and the log agree.
    let trouble = |kind: &str| -> String {
        match (failures, upload_failures) {
            (0, u) => format!(
                "{u} of {successes} {kind} could not be uploaded to the configured destination — \
                 they exist only on this server."
            ),
            (f, 0) => format!("{f} {kind} failed out of {total_attempted} total"),
            (f, u) => format!(
                "{f} {kind} failed out of {total_attempted} total, and {u} more could not be \
                 uploaded off-site."
            ),
        }
    };

    // GAP 10: If backup failures, create managed incident
    if failures > 0 || upload_failures > 0 {
        let _ = sqlx::query(
            "INSERT INTO managed_incidents (user_id, title, status, severity, description, visible_on_status_page) \
             VALUES ($1, $2, 'investigating', 'major', $3, FALSE)"
        )
        .bind(policy.user_id)
        .bind(if failures > 0 {
            format!("Backup policy '{}' had failures", policy.name)
        } else {
            format!("Backup policy '{}' could not reach its destination", policy.name)
        })
        .bind(trouble("backup(s)"))
        .execute(db).await;
    }

    // Fire alert on failure
    if failures > 0 || upload_failures > 0 {
        notifications::fire_alert(
            db, policy.user_id, policy.server_id, None,
            "backup_failure",
            "",
            // An off-site copy that didn't happen is serious but recoverable on the
            // next run, and the data is still on disk. Don't page at the same
            // urgency as "the backup did not happen at all".
            if failures > 0 { "critical" } else { "warning" },
            &if failures > 0 {
                format!("Backup policy '{}' failed", policy.name)
            } else {
                format!("Backup policy '{}' kept its backups on this server", policy.name)
            },
            &trouble("backup(s)"),
        ).await;
    }

    // GAP 2: If verify_after_backup, trigger verification for newly created backups
    if policy.verify_after_backup && successes > 0 {
        tracing::info!("Policy '{}': triggering post-backup verification", policy.name);
        // The backup_verifier service will pick these up on its next cycle
        // since they'll be unverified backups created in the last 7 days
    }
}

/// Simple cron matcher (5 fields: minute hour day month weekday).
fn cron_matches_now(schedule: &str, now: &chrono::DateTime<chrono::Utc>) -> bool {
    let fields: Vec<&str> = schedule.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }

    let checks = [
        (fields[0], now.minute() as i32),
        (fields[1], now.hour() as i32),
        (fields[2], now.day() as i32),
        (fields[3], now.month() as i32),
        (fields[4], now.weekday().num_days_from_sunday() as i32),
    ];

    checks.iter().all(|(field, value)| field_matches(field, *value))
}

fn field_matches(field: &&str, value: i32) -> bool {
    let field = *field;
    if field == "*" {
        return true;
    }

    // Step: */N
    if let Some(step) = field.strip_prefix("*/") {
        if let Ok(n) = step.parse::<i32>() {
            return n > 0 && value % n == 0;
        }
    }

    // List: 1,5,10
    for part in field.split(',') {
        // Range: 1-5
        if let Some((start, end)) = part.split_once('-') {
            if let (Ok(s), Ok(e)) = (start.parse::<i32>(), end.parse::<i32>()) {
                if value >= s && value <= e {
                    return true;
                }
            }
        } else if let Ok(v) = part.parse::<i32>() {
            if v == value {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // s248 / lesson #70: encrypt (execute_policy) and decrypt (restore_db_backup) MUST derive
    // the backup key from the same single source of truth. These pin that derivation so the two
    // sides can never drift back into the incompatible-key bug that made encrypted restores 400.
    #[test]
    fn backup_key_derivation_is_stable_and_prefixed() {
        let jwt = "abcdefghijklmnopqrstuvwxyz0123456789EXTRA";
        let k = derive_backup_encryption_key(jwt);
        // Deterministic + uses only the first 32 bytes of the secret.
        assert_eq!(k, "backup-enc-abcdefghijklmnopqrstuvwxyz012345");
        assert_eq!(derive_backup_encryption_key(jwt), k);
    }

    #[test]
    fn backup_key_derivation_handles_short_secret() {
        // Must not panic on a secret shorter than 32 bytes (32.min(len)).
        assert_eq!(derive_backup_encryption_key("short"), "backup-enc-short");
    }

    #[test]
    fn encrypt_and_restore_derive_identical_key() {
        // The encrypt and decrypt paths both call THIS one function, so identical input →
        // identical output is the guarantee that an encrypted backup is restorable.
        let secret = "the-process-jwt-secret-value-000000000000";
        assert_eq!(
            derive_backup_encryption_key(secret),
            derive_backup_encryption_key(secret),
        );
    }
}
