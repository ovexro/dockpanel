//! Phase 4 W1.2.d: Drill scheduler — runs every 60s, evaluates per-policy
//! drill cron schedules, dispatches end-to-end drills against the latest
//! db + volume backup tied to each due policy.
//!
//! Site backups don't carry `policy_id`, so they're not covered by this
//! scheduler — `backup_verifier::run` (every 6h) handles passive site
//! verification instead. If site policy_id ever lands, this loop should
//! pick up sites with no other change.

use chrono::{Datelike, Timelike};
use sqlx::PgPool;
use uuid::Uuid;

use crate::services::agent::{AgentHandle, AgentRegistry};

#[derive(sqlx::FromRow)]
struct DuePolicy {
    id: Uuid,
    name: String,
    drill_schedule: String,
    last_drill_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Fallback host when a backup row carries no server of its own. Nullable, and
    /// NULL is the only value the UI can produce — the policy form has no server
    /// picker — so it is resolved to the local server explicitly rather than being
    /// allowed to mean "whichever box happens to run the scheduler".
    server_id: Option<Uuid>,
}

/// The policy query is fleet-wide, so this takes the REGISTRY: a drill restores a
/// backup on the server that owns it. A drill that runs on the wrong host certifies
/// disaster recovery for a machine it never touched — and because the Drills tab has
/// no server column and nothing alerts on drill outcomes, that certification is the
/// only artifact anyone sees. It also feeds the chain-of-trust compliance report.
pub async fn run(
    db: PgPool,
    agents: AgentRegistry,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    tracing::info!("Drill scheduler started");

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(e) = tick(&db, &agents).await {
                    tracing::error!("Drill scheduler error: {e}");
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Drill scheduler shutting down gracefully");
                break;
            }
        }
    }
}

async fn tick(db: &PgPool, agents: &AgentRegistry) -> Result<(), String> {
    let now = chrono::Utc::now();

    let policies: Vec<DuePolicy> = sqlx::query_as(
        "SELECT id, name, drill_schedule, last_drill_at, server_id \
         FROM backup_policies \
         WHERE enabled = TRUE AND drill_enabled = TRUE"
    )
    .fetch_all(db).await.map_err(|e| e.to_string())?;

    for policy in &policies {
        if !cron_matches_now(&policy.drill_schedule, &now) {
            continue;
        }
        // 90s debounce — same window as backup_policy_executor.
        if let Some(last) = policy.last_drill_at {
            if (now - last).num_seconds() < 90 {
                continue;
            }
        }

        tracing::info!("Drill schedule due for policy '{}' ({})", policy.name, policy.id);

        // Stamp last_drill_at first so a slow agent dispatch can't double-fire
        // on the next tick.
        let _ = sqlx::query(
            "UPDATE backup_policies SET last_drill_at = NOW() WHERE id = $1"
        ).bind(policy.id).execute(db).await;

        dispatch_policy_drills(db, agents, policy).await;
    }

    Ok(())
}

/// Dispatch this policy's db + volume drills together.
///
/// The guard is consulted ONCE, here, for both legs — and that ordering is
/// load-bearing rather than tidiness. It used to be consulted inside each leg, which
/// was harmless only because `server_id` was always NULL and the guard returned
/// "nothing running" for NULL. Populating `server_id` (the whole point of this change)
/// ARMS the guard, and the db leg inserts its own `running` row before the volume leg
/// looks — so the volume leg would find the db drill it had just triggered, log
/// "another drill running on same server", and return. Volume drills would never run
/// again, once per week, silently. Checking once per policy keeps the cap the comment
/// always claimed (one policy's drills in flight per server) without a leg starving
/// its own sibling.
async fn dispatch_policy_drills(db: &PgPool, agents: &AgentRegistry, policy: &DuePolicy) {
    let db_row: Option<(Uuid, Option<Uuid>, String, String, String)> = sqlx::query_as(
        "SELECT id, server_id, db_type, db_name, filename \
         FROM database_backups \
         WHERE policy_id = $1 \
         ORDER BY created_at DESC LIMIT 1"
    ).bind(policy.id).fetch_optional(db).await.ok().flatten();

    let vol_row: Option<(Uuid, Option<Uuid>, String, String)> = sqlx::query_as(
        "SELECT id, server_id, container_name, filename \
         FROM volume_backups \
         WHERE policy_id = $1 \
         ORDER BY created_at DESC LIMIT 1"
    ).bind(policy.id).fetch_optional(db).await.ok().flatten();

    if db_row.is_none() && vol_row.is_none() {
        tracing::debug!("No backups tied to policy {}; skipping drills", policy.id);
        return;
    }

    // Which host owns this policy's drills. A backup row's own `server_id` wins; the
    // policy's is the fallback; and when both are absent the LOCAL server is named
    // explicitly. Naming it is what makes the concurrency guard and the audit row work
    // at all — `for_server_or_local` would reach the same agent while leaving the row
    // NULL, preserving both bugs.
    let server_id = match db_row.as_ref().and_then(|r| r.1)
        .or_else(|| vol_row.as_ref().and_then(|r| r.1))
        .or(policy.server_id)
    {
        Some(sid) => sid,
        None => match agents.local_server_id().await {
            Some(sid) => sid,
            None => {
                tracing::warn!(
                    "Policy {} has no server and the local server id is not yet known \
                     (setup incomplete) — skipping drills this tick.", policy.id
                );
                return;
            }
        },
    };

    let agent = match agents.for_server(server_id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(
                "Skipping drills for policy {} — server {server_id} is unreachable ({e}). \
                 Not drilling on a different host: a drill that runs elsewhere certifies \
                 disaster recovery for a machine it never tested.", policy.id
            );
            return;
        }
    };

    if has_running_drill_for_server(db, server_id).await {
        tracing::info!(
            "Skipping drills for policy {} — another drill is already running on server {server_id}",
            policy.id
        );
        return;
    }

    if let Some((backup_id, _, db_type, db_name, filename)) = db_row {
        enqueue_drill(
            db, &agent, "database", backup_id, server_id,
            serde_json::json!({
                "db_type": db_type,
                "db_name": db_name,
                "filename": filename,
            }),
            "/backups/drill/db",
        ).await;
    }

    if let Some((backup_id, _, container_name, filename)) = vol_row {
        enqueue_drill(
            db, &agent, "volume", backup_id, server_id,
            serde_json::json!({
                "container_name": container_name,
                "filename": filename,
            }),
            "/backups/drill/volume",
        ).await;
    }
}

/// True if a drill is already in flight for this server.
///
/// The age bound is not cosmetic. The completion UPDATE runs in a detached
/// `tokio::spawn` with no timeout, so a panel restart mid-drill strands a row at
/// `running` for ever. With `server_id` NULL this guard never fired and the leak was
/// invisible; now that it fires, an unbounded count would let ONE stranded row disable
/// DR verification for that machine permanently. A drill that has been "running" for
/// over two hours is not running.
async fn has_running_drill_for_server(db: &PgPool, sid: Uuid) -> bool {
    let count: Option<(i64,)> = sqlx::query_as(
        "SELECT COUNT(*) FROM backup_drills \
         WHERE server_id = $1 AND status IN ('pending', 'running') \
         AND started_at > NOW() - INTERVAL '2 hours'"
    ).bind(sid).fetch_one(db).await.ok();
    count.map(|(n,)| n > 0).unwrap_or(false)
}

/// Insert a `running` row in backup_drills and fire the agent call async.
/// Mirrors the trigger_drill route handler's shape so on-demand and scheduled
/// drills end up indistinguishable in the audit trail (backup_drills + Drills tab).
async fn enqueue_drill(
    db: &PgPool,
    agent: &AgentHandle,
    backup_type: &str,
    backup_id: Uuid,
    server_id: Uuid,
    body: serde_json::Value,
    agent_path: &str,
) {
    // INSERT … RETURNING the new id. triggered_by NULL = scheduler-fired.
    let drill_row: Result<(Uuid,), _> = sqlx::query_as(
        "INSERT INTO backup_drills (backup_type, backup_id, server_id, status, started_at) \
         VALUES ($1, $2, $3, 'running', NOW()) RETURNING id"
    )
    .bind(backup_type).bind(backup_id).bind(server_id)
    .fetch_one(db).await;

    let drill_id = match drill_row {
        Ok((id,)) => id,
        Err(e) => {
            tracing::error!("Failed to insert scheduled drill row: {e}");
            return;
        }
    };

    let agent = agent.clone();
    let db = db.clone();
    let agent_path = agent_path.to_string();
    tokio::spawn(async move {
        let result = agent.post(&agent_path, Some(body)).await.map_err(|e| e.to_string());

        match result {
            Ok(data) => {
                let passed = data.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
                let http_status = data.get("http_status").and_then(|v| v.as_i64()).map(|n| n as i32);
                let body_excerpt = data.get("body_excerpt").and_then(|v| v.as_str()).map(|s| s.to_string());
                let error_message = data.get("error_message").and_then(|v| v.as_str()).map(|s| s.to_string());
                let duration_ms = data.get("duration_ms").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

                let _ = sqlx::query(
                    "UPDATE backup_drills SET \
                     status = $2, http_status = $3, body_excerpt = $4, \
                     error_message = $5, duration_ms = $6, completed_at = NOW() \
                     WHERE id = $1"
                )
                .bind(drill_id)
                .bind(if passed { "passed" } else { "failed" })
                .bind(http_status).bind(body_excerpt)
                .bind(error_message).bind(duration_ms)
                .execute(&db).await;
            }
            Err(e) => {
                let _ = sqlx::query(
                    "UPDATE backup_drills SET status = 'failed', error_message = $2, completed_at = NOW() WHERE id = $1"
                ).bind(drill_id).bind(&e).execute(&db).await;
            }
        }
    });
}

// ── 5-field cron parser (parity with backup_policy_executor) ────────────────

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
    checks.iter().all(|(f, v)| field_matches(f, *v))
}

fn field_matches(field: &str, value: i32) -> bool {
    if field == "*" {
        return true;
    }
    if let Some(stripped) = field.strip_prefix("*/") {
        if let Ok(step) = stripped.parse::<i32>() {
            return step > 0 && value % step == 0;
        }
        return false;
    }
    field.split(',').any(|part| {
        if let Some((lo, hi)) = part.split_once('-') {
            match (lo.parse::<i32>(), hi.parse::<i32>()) {
                (Ok(l), Ok(h)) => value >= l && value <= h,
                _ => false,
            }
        } else {
            part.parse::<i32>().map(|n| n == value).unwrap_or(false)
        }
    })
}
