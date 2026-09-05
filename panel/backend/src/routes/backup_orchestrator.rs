use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, paginate, require_admin, ApiError};
use crate::services::activity;
use crate::services::extensions::fire_event;
use crate::AppState;

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct BackupPolicy {
    pub id: Uuid,
    pub user_id: Uuid,
    pub server_id: Option<Uuid>,
    pub name: String,
    pub backup_sites: bool,
    pub backup_databases: bool,
    pub backup_volumes: bool,
    pub schedule: String,
    pub destination_id: Option<Uuid>,
    pub retention_count: i32,
    pub encrypt: bool,
    pub verify_after_backup: bool,
    pub enabled: bool,
    pub drill_enabled: bool,
    pub drill_schedule: String,
    pub last_drill_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_run: Option<chrono::DateTime<chrono::Utc>>,
    pub last_status: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreatePolicyRequest {
    pub name: String,
    pub server_id: Option<Uuid>,
    pub backup_sites: Option<bool>,
    pub backup_databases: Option<bool>,
    pub backup_volumes: Option<bool>,
    pub schedule: Option<String>,
    pub destination_id: Option<Uuid>,
    pub retention_count: Option<i32>,
    pub encrypt: Option<bool>,
    pub verify_after_backup: Option<bool>,
    pub enabled: Option<bool>,
    pub drill_enabled: Option<bool>,
    pub drill_schedule: Option<String>,
}

/// Distinguish "the caller omitted this field" from "the caller sent null".
///
/// `destination_id` needs both: omitting it must preserve the attached
/// destination, while sending `null` is how the operator detaches one and goes
/// back to local-only. With a plain `Option` the two are the same value, which
/// is why a partial PUT used to silently NULL the very destination the
/// preflight warning asks the operator to attach.
fn de_double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    serde::Deserialize::deserialize(d).map(Some)
}

/// Body of PUT /policies/{id}. Separate from [`CreatePolicyRequest`] because on
/// create an absent field means "use the default", while on update it means
/// "leave it as it is" — the same `None` standing for two different intentions
/// is what made this endpoint destructive.
#[derive(serde::Deserialize)]
pub struct UpdatePolicyRequest {
    pub name: String,
    #[serde(default, deserialize_with = "de_double_option")]
    pub server_id: Option<Option<Uuid>>,
    pub backup_sites: Option<bool>,
    pub backup_databases: Option<bool>,
    pub backup_volumes: Option<bool>,
    pub schedule: Option<String>,
    #[serde(default, deserialize_with = "de_double_option")]
    pub destination_id: Option<Option<Uuid>>,
    pub retention_count: Option<i32>,
    pub encrypt: Option<bool>,
    pub verify_after_backup: Option<bool>,
    pub enabled: Option<bool>,
    pub drill_enabled: Option<bool>,
    pub drill_schedule: Option<String>,
}

/// Reject obviously-invalid cron strings before they hit the DB. The
/// scheduler's parser is 5-field whitespace-separated; same shape as the
/// existing backup_policy_executor.
fn is_valid_cron_5(field: &str) -> bool {
    let parts: Vec<&str> = field.split_whitespace().collect();
    parts.len() == 5 && parts.iter().all(|p| !p.is_empty() && p.len() <= 32)
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct DatabaseBackup {
    pub id: Uuid,
    pub database_id: Uuid,
    pub server_id: Option<Uuid>,
    pub filename: String,
    pub size_bytes: i64,
    pub db_type: String,
    pub db_name: String,
    pub encrypted: bool,
    /// Which `derive_backup_encryption_key_for_version` derivation actually
    /// produced this file's passphrase — restore MUST use this, not
    /// `CURRENT_BACKUP_KEY_VERSION`, or a backup written under an older
    /// derivation becomes permanently unrestorable the moment the
    /// derivation changes again.
    pub encryption_key_version: i16,
    pub uploaded: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub sha256_hash: Option<String>,
    pub previous_hash: Option<String>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct VolumeBackup {
    pub id: Uuid,
    pub container_id: String,
    pub container_name: String,
    pub server_id: Option<Uuid>,
    pub volume_name: String,
    pub filename: String,
    pub size_bytes: i64,
    pub encrypted: bool,
    pub uploaded: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub sha256_hash: Option<String>,
    pub previous_hash: Option<String>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct BackupVerification {
    pub id: Uuid,
    pub backup_type: String,
    pub backup_id: Uuid,
    pub server_id: Option<Uuid>,
    pub status: String,
    pub checks_run: i32,
    pub checks_passed: i32,
    pub details: serde_json::Value,
    pub error_message: Option<String>,
    pub duration_ms: Option<i32>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct BackupDrill {
    pub id: Uuid,
    pub backup_type: String,
    pub backup_id: Uuid,
    pub server_id: Option<Uuid>,
    pub triggered_by: Option<Uuid>,
    pub status: String,
    pub http_status: Option<i32>,
    pub body_excerpt: Option<String>,
    pub error_message: Option<String>,
    pub duration_ms: Option<i32>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct PaginationQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

// ── Unified Backup View (fleet-wide) ────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct UnifiedBackupsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub kind: Option<String>,
    pub server_id: Option<Uuid>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct UnifiedBackupRow {
    pub id: Uuid,
    pub kind: String,
    pub resource_id: Option<Uuid>,
    pub resource_name: String,
    pub filename: String,
    pub size_bytes: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub server_id: Option<Uuid>,
    pub server_name: String,
    pub server_is_local: bool,
    pub encrypted: bool,
    pub uploaded: bool,
    pub extra_type: Option<String>,
}

#[derive(serde::Serialize)]
pub struct UnifiedBackupsResponse {
    pub items: Vec<UnifiedBackupRow>,
    pub total: i64,
}

// ── Health Dashboard ────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct BackupHealth {
    pub total_site_backups: i64,
    pub total_db_backups: i64,
    pub total_volume_backups: i64,
    pub total_storage_bytes: i64,
    pub last_24h_success: i64,
    pub last_24h_failed: i64,
    pub policies_active: i64,
    pub policies_total: i64,
    pub verifications_passed: i64,
    pub verifications_failed: i64,
    pub oldest_unverified_days: Option<i64>,
    pub stale_backups: Vec<StaleBackup>,
    // SLA windowed view (W1.1): "of the last N backups, how many are verified?"
    pub sla_window: i64,
    pub sla_verified: i64,
    pub sla_failed: i64,
    pub sla_pending: i64,
    pub verify_lag_p50_hours: Option<f64>,
    pub verify_lag_p95_hours: Option<f64>,
    pub per_server_sla: Vec<ServerSla>,
    /// The SLA figures above could not be computed. Zeroes then mean "not
    /// measured", NOT "nothing to measure" — the two render identically
    /// otherwise, and for six releases the card showed its empty state on every
    /// install because a query referenced a column that does not exist.
    pub sla_unavailable: bool,
    // Drill counts (Phase 4 W1.2): end-to-end restore probes in last 30d.
    pub drills_passed_30d: i64,
    pub drills_failed_30d: i64,
}

#[derive(serde::Serialize)]
pub struct StaleBackup {
    pub resource_type: String,
    pub resource_name: String,
    /// None = this resource has never been backed up. That is the worst case the
    /// list reports, so it must be representable rather than a decode failure.
    pub last_backup: Option<chrono::DateTime<chrono::Utc>>,
    pub days_since: Option<i64>,
}

#[derive(serde::Serialize)]
pub struct ServerSla {
    pub server_id: Option<Uuid>,
    pub server_name: String,
    pub verified: i64,
    pub total: i64,
    pub lag_p95_hours: Option<f64>,
}

/// GET /api/backup-orchestrator/all — Unified fleet-wide backup list across site, database, and volume backups.
///
/// Scoped, not truly fleet-wide despite the name: an admin sees the local server plus any
/// server THEY registered, same predicate `list_volume_backups`/`chain_report` use (v2.184.0
/// / this session). Before this fix `AdminUser` alone gated it — a flat, unscoped CTE handing
/// every admin account the backup ids and resource names (site domains, db/container:volume
/// names, filenames, sizes) of the ENTIRE fleet, including servers another admin registered.
/// That id was also the exact discovery channel a caller needed to walk into the (now also
/// fixed) unscoped chain-report endpoints.
///
/// Optional filters: `kind` (site|database|volume) and `server_id`.
/// Site backups derive their server via `sites.server_id`; database and volume backups carry
/// `server_id` directly (nullable — a NULL row predates the fleet backfill and is treated as
/// local, matching the COALESCE default below; it is visible to every admin the same as any
/// other local-server row, never scoped away).
pub async fn list_all_backups(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Query(params): Query<UnifiedBackupsQuery>,
) -> Result<Json<UnifiedBackupsResponse>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let kind_filter = match params.kind.as_deref() {
        None => None,
        Some("site") | Some("database") | Some("volume") => params.kind.clone(),
        Some(_) => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "kind must be one of: site, database, volume",
            ));
        }
    };

    // CTE unions the three backup tables into a common shape.
    // - Site backups: server_id derived from sites (backups table has no server_id column).
    // - Database backups: server_id nullable on table; falls through to local via LEFT JOIN.
    // - Volume backups: server_id nullable on table.
    let cte = "WITH unified AS ( \
         SELECT b.id, 'site'::text AS kind, b.site_id AS resource_id, s.domain AS resource_name, \
                b.filename, b.size_bytes, b.created_at, s.server_id, \
                FALSE AS encrypted, b.uploaded, NULL::text AS extra_type \
           FROM backups b JOIN sites s ON s.id = b.site_id \
         UNION ALL \
         SELECT db.id, 'database'::text, db.database_id, db.db_name, \
                db.filename, db.size_bytes, db.created_at, db.server_id, \
                db.encrypted, db.uploaded, db.db_type \
           FROM database_backups db \
         UNION ALL \
         SELECT vb.id, 'volume'::text, NULL::uuid, \
                (vb.container_name || ':' || vb.volume_name) AS resource_name, \
                vb.filename, vb.size_bytes, vb.created_at, vb.server_id, \
                vb.encrypted, vb.uploaded, NULL::text \
           FROM volume_backups vb \
       )";

    let list_sql = format!(
        "{cte} SELECT u.id, u.kind, u.resource_id, u.resource_name, u.filename, u.size_bytes, \
                u.created_at, u.server_id, \
                COALESCE(srv.name, 'local') AS server_name, \
                COALESCE(srv.is_local, TRUE) AS server_is_local, \
                u.encrypted, u.uploaded, u.extra_type \
           FROM unified u LEFT JOIN servers srv ON srv.id = u.server_id \
          WHERE ($1::uuid IS NULL OR u.server_id = $1) \
            AND ($2::text IS NULL OR u.kind = $2) \
            AND (u.server_id IS NULL OR srv.is_local OR srv.user_id = $5) \
          ORDER BY u.created_at DESC LIMIT $3 OFFSET $4"
    );

    let items: Vec<UnifiedBackupRow> = sqlx::query_as(&list_sql)
        .bind(params.server_id)
        .bind(&kind_filter)
        .bind(limit)
        .bind(offset)
        .bind(claims.sub)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list all backups", e))?;

    let count_sql = format!(
        "{cte} SELECT COUNT(*)::bigint FROM unified u LEFT JOIN servers srv ON srv.id = u.server_id \
          WHERE ($1::uuid IS NULL OR u.server_id = $1) \
            AND ($2::text IS NULL OR u.kind = $2) \
            AND (u.server_id IS NULL OR srv.is_local OR srv.user_id = $3) \
          "
    );

    let (total,): (i64,) = sqlx::query_as(&count_sql)
        .bind(params.server_id)
        .bind(&kind_filter)
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("list all backups count", e))?;

    Ok(Json(UnifiedBackupsResponse { items, total }))
}

/// GET /api/backup-orchestrator/health — Global backup health dashboard.
/// GET /api/backup-orchestrator/preflight — Backup prerequisites.
///
/// Answers the question the Overview tab could not: will any of this survive
/// losing the machine? One result per enabled policy, plus the prior question of
/// whether anything is scheduled at all.
pub async fn preflight(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::services::prerequisites::backups::{
        check_any_policy_enabled, check_backup_destination, PolicyDestinationFacts,
    };

    let db = &state.db;

    let (sites,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sites")
        .fetch_one(db).await.map_err(|e| internal_error("preflight", e))?;
    let (destinations,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_destinations")
        .fetch_one(db).await.map_err(|e| internal_error("preflight", e))?;

    // LEFT JOIN so a policy pointing at a deleted destination is distinguishable
    // from one that never chose: `destination_id` set with a NULL name.
    let policies: Vec<(String, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT p.name, p.destination_id, d.name \
           FROM backup_policies p \
           LEFT JOIN backup_destinations d ON d.id = p.destination_id \
          WHERE p.enabled = TRUE ORDER BY p.created_at",
    )
    .fetch_all(db)
    .await
    .map_err(|e| internal_error("preflight policies", e))?;

    let mut checks = vec![check_any_policy_enabled(policies.len(), sites as usize)];

    for (name, destination_id, destination_name) in policies {
        checks.push(check_backup_destination(&PolicyDestinationFacts {
            policy_name: name,
            destination_exists: destination_id.is_some() && destination_name.is_some(),
            destination_id,
            destination_name,
            destinations_available: destinations as usize,
        }));
    }

    Ok(Json(serde_json::json!({ "checks": checks })))
}

pub async fn health(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<BackupHealth>, ApiError> {
    let db = &state.db;

    let (total_site,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backups")
        .fetch_one(db).await.map_err(|e| internal_error("health", e))?;
    let (total_db,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM database_backups")
        .fetch_one(db).await.map_err(|e| internal_error("health", e))?;
    let (total_vol,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM volume_backups")
        .fetch_one(db).await.map_err(|e| internal_error("health", e))?;

    // SUM(BIGINT) returns NUMERIC in postgres — cast back to bigint so sqlx can
    // decode into Option<i64>. Empty rowsets give NULL, populated ones used to
    // 500 with "INT8 not compatible with NUMERIC" until this cast was added.
    let (site_storage,): (Option<i64>,) = sqlx::query_as("SELECT SUM(size_bytes)::bigint FROM backups")
        .fetch_one(db).await.map_err(|e| internal_error("health", e))?;
    let (db_storage,): (Option<i64>,) = sqlx::query_as("SELECT SUM(size_bytes)::bigint FROM database_backups")
        .fetch_one(db).await.map_err(|e| internal_error("health", e))?;
    let (vol_storage,): (Option<i64>,) = sqlx::query_as("SELECT SUM(size_bytes)::bigint FROM volume_backups")
        .fetch_one(db).await.map_err(|e| internal_error("health", e))?;

    let total_storage = site_storage.unwrap_or(0) + db_storage.unwrap_or(0) + vol_storage.unwrap_or(0);

    // Count successful schedules in last 24h
    let (success_24h,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM backup_schedules WHERE last_status = 'success' AND last_run > NOW() - INTERVAL '24 hours'"
    ).fetch_one(db).await.map_err(|e| internal_error("health", e))?;
    let (failed_24h,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM backup_schedules WHERE last_status = 'failed' AND last_run > NOW() - INTERVAL '24 hours'"
    ).fetch_one(db).await.map_err(|e| internal_error("health", e))?;

    let (policies_active,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_policies WHERE enabled = TRUE")
        .fetch_one(db).await.map_err(|e| internal_error("health", e))?;
    let (policies_total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_policies")
        .fetch_one(db).await.map_err(|e| internal_error("health", e))?;

    let (verif_passed,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_verifications WHERE status = 'passed'")
        .fetch_one(db).await.map_err(|e| internal_error("health", e))?;
    let (verif_failed,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_verifications WHERE status = 'failed'")
        .fetch_one(db).await.map_err(|e| internal_error("health", e))?;

    // Find stale sites (no backup in > 7 days)
    //
    // The join admits only archives that landed where they were meant to. An
    // upload that exhausted its retries still writes a row — that row is what
    // makes the local file reapable — but it is not off-site, so counting it as
    // a fresh backup would quietly disarm this list for exactly the site whose
    // destination is down.
    //
    // `last_backup` is Option because the HAVING clause deliberately admits sites
    // that have NEVER been backed up — those are the ones that matter most — and
    // a LEFT JOIN miss makes MAX() NULL. Decoding that into a bare DateTime made
    // `fetch_all` fail on the FIRST row (NULLS FIRST sorts them to the front),
    // and `unwrap_or_default` then blanked the entire warning, including the
    // genuinely-stale sites that decoded fine.
    let stale_sites: Vec<(String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT s.domain, MAX(b.created_at) as last_backup \
         FROM sites s LEFT JOIN backups b ON b.site_id = s.id \
                                        AND (b.destination_id IS NULL OR b.uploaded) \
         GROUP BY s.id, s.domain \
         HAVING MAX(b.created_at) IS NULL OR MAX(b.created_at) < NOW() - INTERVAL '7 days' \
         ORDER BY MAX(b.created_at) NULLS FIRST LIMIT 10"
    ).fetch_all(db).await
    .map_err(|e| { tracing::error!("backup health: stale-site query failed: {e}"); e })
    .unwrap_or_default();

    let now = chrono::Utc::now();
    let stale_backups: Vec<StaleBackup> = stale_sites.into_iter().map(|(domain, last)| {
        StaleBackup {
            resource_type: "site".into(),
            resource_name: domain,
            // None = never backed up at all. `days_since` is left null rather
            // than faked from epoch so the UI can say so instead of printing an
            // arithmetic artefact like "20789 days".
            days_since: last.map(|l| (now - l).num_days()),
            last_backup: last,
        }
    }).collect();

    // ── SLA windowed view (W1.1) ────────────────────────────────────────────
    // "Of the last N backups across all kinds, how many are verified?"
    // Latest verification per (kind, backup_id) wins (re-verifications supersede).
    const SLA_WINDOW: i64 = 30;
    // `backups` has NO server_id column — a site reaches its server through
    // `sites`, exactly as the unified list query above does. Selecting it
    // directly made this statement fail at PLAN time, on every request, on every
    // install, from the day the card shipped.
    let mut sla_unavailable = false;
    let (sla_verified, sla_failed, sla_pending): (i64, i64, i64) = sqlx::query_as(
        "WITH all_backups AS ( \
            SELECT b.id, b.created_at, s.server_id, 'site'::text AS kind \
              FROM backups b JOIN sites s ON s.id = b.site_id \
            UNION ALL \
            SELECT id, created_at, server_id, 'database'::text FROM database_backups \
            UNION ALL \
            SELECT id, created_at, server_id, 'volume'::text FROM volume_backups \
         ), recent AS ( \
            SELECT * FROM all_backups ORDER BY created_at DESC LIMIT $1 \
         ), latest_verif AS ( \
            SELECT DISTINCT ON (bv.backup_type, bv.backup_id) bv.backup_type, bv.backup_id, bv.status \
            FROM backup_verifications bv \
            JOIN recent r ON r.id = bv.backup_id AND r.kind = bv.backup_type \
            ORDER BY bv.backup_type, bv.backup_id, bv.created_at DESC \
         ) \
         SELECT \
            COUNT(*) FILTER (WHERE lv.status = 'passed')::bigint AS verified, \
            COUNT(*) FILTER (WHERE lv.status = 'failed')::bigint AS failed, \
            COUNT(*) FILTER (WHERE lv.status IS NULL OR lv.status IN ('pending','running'))::bigint AS pending \
         FROM recent r \
         LEFT JOIN latest_verif lv ON lv.backup_type = r.kind AND lv.backup_id = r.id"
    )
    .bind(SLA_WINDOW)
    .fetch_one(db).await
    .unwrap_or_else(|e| {
        tracing::error!("backup health: SLA window query failed: {e}");
        sla_unavailable = true;
        (0, 0, 0)
    });

    // Verify lag percentiles: hours between backup creation and verification completion,
    // for backups created in the last 30 days that have a passed verification.
    let (lag_p50, lag_p95): (Option<f64>, Option<f64>) = sqlx::query_as(
        "WITH all_backups AS ( \
            SELECT id, created_at, 'site'::text AS kind FROM backups \
            UNION ALL \
            SELECT id, created_at, 'database'::text FROM database_backups \
            UNION ALL \
            SELECT id, created_at, 'volume'::text FROM volume_backups \
         ), lags AS ( \
            SELECT EXTRACT(EPOCH FROM (bv.completed_at - ab.created_at)) / 3600.0 AS hours \
            FROM backup_verifications bv \
            JOIN all_backups ab ON ab.id = bv.backup_id AND ab.kind = bv.backup_type \
            WHERE bv.status = 'passed' \
              AND bv.completed_at IS NOT NULL \
              AND ab.created_at > NOW() - INTERVAL '30 days' \
         ) \
         SELECT \
            PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY hours)::float8 AS p50, \
            PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY hours)::float8 AS p95 \
         FROM lags"
    )
    .fetch_one(db).await
    .unwrap_or((None, None));

    // Oldest unverified backup age in days (across all kinds, ignoring backups > 90d old).
    let oldest_unverified_days: Option<i64> = sqlx::query_scalar(
        "WITH all_backups AS ( \
            SELECT id, created_at, 'site'::text AS kind FROM backups \
            UNION ALL \
            SELECT id, created_at, 'database'::text FROM database_backups \
            UNION ALL \
            SELECT id, created_at, 'volume'::text FROM volume_backups \
         ) \
         SELECT EXTRACT(DAY FROM NOW() - MIN(ab.created_at))::bigint \
         FROM all_backups ab \
         LEFT JOIN backup_verifications bv \
              ON bv.backup_id = ab.id AND bv.backup_type = ab.kind AND bv.status = 'passed' \
         WHERE bv.id IS NULL \
           AND ab.created_at > NOW() - INTERVAL '90 days'"
    )
    .fetch_one(db).await
    .ok()
    .flatten();

    // Per-server SLA: same windowed view, grouped by server_id.
    // Bigger window (90d) here so the breakdown isn't dominated by whichever
    // server happened to back up most recently.
    let per_server_rows: Vec<(Option<Uuid>, String, i64, i64, Option<f64>)> = sqlx::query_as(
        "WITH all_backups AS ( \
            SELECT b.id, b.created_at, s.server_id, 'site'::text AS kind \
              FROM backups b JOIN sites s ON s.id = b.site_id \
            UNION ALL \
            SELECT id, created_at, server_id, 'database'::text FROM database_backups \
            UNION ALL \
            SELECT id, created_at, server_id, 'volume'::text FROM volume_backups \
         ), recent AS ( \
            SELECT * FROM all_backups WHERE created_at > NOW() - INTERVAL '30 days' \
         ), latest_verif AS ( \
            SELECT DISTINCT ON (bv.backup_type, bv.backup_id) \
                   bv.backup_type, bv.backup_id, bv.status, bv.completed_at \
            FROM backup_verifications bv \
            JOIN recent r ON r.id = bv.backup_id AND r.kind = bv.backup_type \
            ORDER BY bv.backup_type, bv.backup_id, bv.created_at DESC \
         ), joined AS ( \
            SELECT r.server_id, r.created_at, lv.status, lv.completed_at \
            FROM recent r \
            LEFT JOIN latest_verif lv ON lv.backup_type = r.kind AND lv.backup_id = r.id \
         ) \
         SELECT \
            j.server_id, \
            COALESCE(s.name, '(local)') AS server_name, \
            COUNT(*) FILTER (WHERE j.status = 'passed')::bigint AS verified, \
            COUNT(*)::bigint AS total, \
            PERCENTILE_CONT(0.95) WITHIN GROUP ( \
                ORDER BY EXTRACT(EPOCH FROM (j.completed_at - j.created_at)) / 3600.0 \
            ) FILTER (WHERE j.status = 'passed' AND j.completed_at IS NOT NULL)::float8 AS lag_p95 \
         FROM joined j \
         LEFT JOIN servers s ON s.id = j.server_id \
         GROUP BY j.server_id, s.name \
         ORDER BY total DESC \
         LIMIT 20"
    )
    .fetch_all(db).await
    .map_err(|e| {
        tracing::error!("backup health: per-server SLA query failed: {e}");
        sla_unavailable = true;
        e
    })
    .unwrap_or_default();

    let per_server_sla: Vec<ServerSla> = per_server_rows.into_iter()
        .map(|(server_id, server_name, verified, total, lag_p95_hours)| ServerSla {
            server_id, server_name, verified, total, lag_p95_hours,
        })
        .collect();

    // Drill counts (last 30d).
    let (drills_passed_30d, drills_failed_30d): (i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE status = 'passed')::bigint, \
            COUNT(*) FILTER (WHERE status = 'failed')::bigint \
         FROM backup_drills \
         WHERE created_at > NOW() - INTERVAL '30 days'"
    )
    .fetch_one(db).await
    .unwrap_or((0, 0));

    Ok(Json(BackupHealth {
        total_site_backups: total_site,
        total_db_backups: total_db,
        total_volume_backups: total_vol,
        total_storage_bytes: total_storage,
        last_24h_success: success_24h,
        last_24h_failed: failed_24h,
        policies_active,
        policies_total,
        verifications_passed: verif_passed,
        verifications_failed: verif_failed,
        oldest_unverified_days,
        stale_backups,
        sla_window: SLA_WINDOW,
        sla_verified,
        sla_failed,
        sla_pending,
        verify_lag_p50_hours: lag_p50,
        verify_lag_p95_hours: lag_p95,
        per_server_sla,
        sla_unavailable,
        drills_passed_30d,
        drills_failed_30d,
    }))
}

// ── Policies CRUD ───────────────────────────────────────────────────────────

/// GET /api/backup-orchestrator/policies — List policies.
pub async fn list_policies(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<BackupPolicy>>, ApiError> {
    require_admin(&claims.role)?;
    let policies: Vec<BackupPolicy> = sqlx::query_as(
        "SELECT * FROM backup_policies WHERE user_id = $1 ORDER BY created_at DESC LIMIT 500"
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list policies", e))?;

    Ok(Json(policies))
}

/// POST /api/backup-orchestrator/policies — Create a policy.
pub async fn create_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(req): Json<CreatePolicyRequest>,
) -> Result<(StatusCode, Json<BackupPolicy>), ApiError> {
    require_admin(&claims.role)?;
    if req.name.is_empty() || req.name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
    }

    let schedule = req.schedule.unwrap_or_else(|| "0 2 * * *".into());
    if !is_valid_cron_5(&schedule) {
        return Err(err(StatusCode::BAD_REQUEST, "schedule must be a 5-field cron string"));
    }
    let drill_schedule = req.drill_schedule.unwrap_or_else(|| "0 4 * * 0".into());
    if !is_valid_cron_5(&drill_schedule) {
        return Err(err(StatusCode::BAD_REQUEST, "drill_schedule must be a 5-field cron string"));
    }
    let retention = req.retention_count.unwrap_or(7).max(1).min(365);

    let policy: BackupPolicy = sqlx::query_as(
        "INSERT INTO backup_policies (user_id, server_id, name, backup_sites, backup_databases, backup_volumes, \
         schedule, destination_id, retention_count, encrypt, verify_after_backup, enabled, \
         drill_enabled, drill_schedule) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) \
         RETURNING *"
    )
    .bind(claims.sub)
    .bind(req.server_id)
    .bind(&req.name)
    .bind(req.backup_sites.unwrap_or(true))
    .bind(req.backup_databases.unwrap_or(true))
    .bind(req.backup_volumes.unwrap_or(false))
    .bind(&schedule)
    .bind(req.destination_id)
    .bind(retention)
    .bind(req.encrypt.unwrap_or(false))
    .bind(req.verify_after_backup.unwrap_or(false))
    .bind(req.enabled.unwrap_or(true))
    .bind(req.drill_enabled.unwrap_or(false))
    .bind(&drill_schedule)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create policy", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "backup_policy.create",
        Some("backup_policy"), Some(&req.name), None, None,
    ).await;

    fire_event(&state.db, "backup_policy.created", serde_json::json!({
        "policy_id": policy.id, "name": &req.name,
    }));

    Ok((StatusCode::CREATED, Json(policy)))
}

/// PUT /api/backup-orchestrator/policies/{id} — Update a policy.
pub async fn update_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdatePolicyRequest>,
) -> Result<Json<BackupPolicy>, ApiError> {
    require_admin(&claims.role)?;
    // Verify ownership, and read the three columns the UPDATE assigns
    // unconditionally so an omitted field can keep its current value instead of
    // being reset.
    let existing: Option<(Uuid, Option<Uuid>, Option<Uuid>, i32)> = sqlx::query_as(
        "SELECT id, server_id, destination_id, retention_count FROM backup_policies WHERE id = $1 AND user_id = $2"
    )
    .bind(id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("update policy", e))?;

    let Some((_, cur_server_id, cur_destination_id, cur_retention)) = existing else {
        return Err(err(StatusCode::NOT_FOUND, "Policy not found"));
    };

    // Omitted -> keep. Explicit null -> clear. A value -> set.
    let server_id = req.server_id.unwrap_or(cur_server_id);
    let destination_id = req.destination_id.unwrap_or(cur_destination_id);
    // `unwrap_or(7)` here meant an edit that did not mention retention silently
    // reset it to seven — the same reset-on-omission class as the two above.
    let retention = req.retention_count.unwrap_or(cur_retention).max(1).min(365);

    if let Some(s) = &req.schedule {
        if !s.is_empty() && !is_valid_cron_5(s) {
            return Err(err(StatusCode::BAD_REQUEST, "schedule must be a 5-field cron string"));
        }
    }
    if let Some(s) = &req.drill_schedule {
        if !s.is_empty() && !is_valid_cron_5(s) {
            return Err(err(StatusCode::BAD_REQUEST, "drill_schedule must be a 5-field cron string"));
        }
    }

    let policy: BackupPolicy = sqlx::query_as(
        "UPDATE backup_policies SET \
         name = COALESCE(NULLIF($2, ''), name), \
         server_id = $3, \
         backup_sites = COALESCE($4, backup_sites), \
         backup_databases = COALESCE($5, backup_databases), \
         backup_volumes = COALESCE($6, backup_volumes), \
         schedule = COALESCE(NULLIF($7, ''), schedule), \
         destination_id = $8, \
         retention_count = $9, \
         encrypt = COALESCE($10, encrypt), \
         verify_after_backup = COALESCE($11, verify_after_backup), \
         enabled = COALESCE($12, enabled), \
         drill_enabled = COALESCE($13, drill_enabled), \
         drill_schedule = COALESCE(NULLIF($14, ''), drill_schedule), \
         updated_at = NOW() \
         WHERE id = $1 RETURNING *"
    )
    .bind(id)
    .bind(&req.name)
    .bind(server_id)
    .bind(req.backup_sites)
    .bind(req.backup_databases)
    .bind(req.backup_volumes)
    .bind(req.schedule.as_deref().unwrap_or(""))
    .bind(destination_id)
    .bind(retention)
    .bind(req.encrypt)
    .bind(req.verify_after_backup)
    .bind(req.enabled)
    .bind(req.drill_enabled)
    .bind(req.drill_schedule.as_deref().unwrap_or(""))
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("update policy", e))?;

    Ok(Json(policy))
}

/// DELETE /api/backup-orchestrator/policies/{id} — Delete a policy.
pub async fn delete_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = sqlx::query("DELETE FROM backup_policies WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub)
        .execute(&state.db).await
        .map_err(|e| internal_error("delete policy", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Policy not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/backup-orchestrator/policies/protect-all — Create a backup-everything policy.
pub async fn protect_all(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;
    let db = &state.db;
    let policy_name = "Protect Everything";

    // Check if already exists
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM backup_policies WHERE user_id = $1 AND name = $2"
    )
    .bind(claims.sub).bind(policy_name)
    .fetch_optional(db).await
    .map_err(|e| internal_error("protect all", e))?;

    if let Some((existing_id,)) = existing {
        return Err(err(StatusCode::CONFLICT,
            &format!("Policy '{}' already exists (id: {})", policy_name, existing_id)));
    }

    let policy: BackupPolicy = sqlx::query_as(
        "INSERT INTO backup_policies (user_id, name, backup_sites, backup_databases, backup_volumes, \
         schedule, retention_count, encrypt, verify_after_backup, enabled) \
         VALUES ($1, $2, TRUE, TRUE, TRUE, '0 2 * * *', 7, FALSE, TRUE, TRUE) \
         RETURNING *"
    )
    .bind(claims.sub)
    .bind(policy_name)
    .fetch_one(db).await
    .map_err(|e| internal_error("protect all", e))?;

    activity::log_activity(
        db, claims.sub, &claims.email, "backup_policy.protect_all",
        Some("backup_policy"), Some(policy_name), None, None,
    ).await;

    fire_event(db, "backup_policy.created", serde_json::json!({
        "policy_id": policy.id, "name": policy_name, "preset": "protect-all",
    }));

    Ok((StatusCode::CREATED, Json(serde_json::json!({
        "id": policy.id,
        "name": policy_name,
        "schedule": "0 2 * * *",
        "backup_sites": true,
        "backup_databases": true,
        "backup_volumes": true,
        "retention_count": 7,
        "verify_after_backup": true,
    }))))
}

// ── Database Backups ────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct CreateDbBackupRequest {
    pub database_id: Uuid,
}

/// POST /api/backup-orchestrator/db-backup — Create a database backup.
pub async fn create_db_backup(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(req): Json<CreateDbBackupRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // Fetch database details (join with sites for user ownership and server_id)
    let row: Option<(Uuid, String, String, String, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT d.id, d.name, d.engine, d.db_user, d.db_password_enc, s.server_id \
         FROM databases d JOIN sites s ON d.site_id = s.id \
         WHERE d.id = $1 AND s.user_id = $2"
    )
    .bind(req.database_id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("create db backup", e))?;

    let (db_id, db_name, engine, user, password_enc, server_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;

    // Dump on the host the SITE lives on. This handler already held the answer: the
    // join above reads `s.server_id` and the INSERT below stamps the new row with it,
    // while the dump itself went to whatever agent the caller's server switcher named
    // (an HTTP header with a local-agent fallback). So the archive was written on host
    // A and the row claimed host B — a guaranteed lie the moment the two disagree, and
    // three consumers read that column to decide where to act: the verifier
    // (`services/backup_verifier.rs`) looks for the file on the named host, the drill
    // scheduler (`services/drill_scheduler.rs`) restores from it there, and the
    // retention pruner in `services/auto_healer.rs` only unlinks a path on the host the
    // row names — so a mislabelled archive is never verified, never drilled, and never
    // pruned, and it grows on a machine nothing is watching.
    //
    // The credential is the sharper half: the body below carries this database's
    // DECRYPTED password, so a stale server selection decided which machine received a
    // secret it has no business holding. Resolving before the decrypt keeps a refusal
    // from ever unwrapping it.
    //
    // `backup_policy_executor` runs this exact dump from the same join and has always
    // resolved `agents.for_server(s.server_id)` for it. The scheduled path was right and
    // the operator-pressed button never adopted it — that asymmetry is the whole defect.
    let agent = crate::helpers::agent_for_site_server(
        &state, server_id, &format!("database {db_name}"),
    ).await?;

    // Decrypt the database password (handles both encrypted and legacy plaintext)
    let password = crate::services::secrets_crypto::decrypt_credential_or_legacy(&password_enc, &state.config.jwt_secret);

    // Container name follows convention: dockpanel-db-{name}
    let container_name = format!("dockpanel-db-{db_name}");

    // Manual (ad-hoc) DB backups are not encrypted: there is no manual-encryption toggle —
    // only policy-driven backups encrypt (via backup_policy_executor). The previous read of
    // backup_destinations.encryption_key was dead (nothing writes that column) and always
    // yielded None, so this preserves the actual behavior while removing the phantom query and
    // keeping `encrypted` truthful. Encrypted policy backups remain restorable via the shared
    // key derivation in restore_db_backup.
    let encryption_key: Option<String> = None;

    // Call agent to dump database
    let body = serde_json::json!({
        "container_name": container_name,
        "db_name": db_name,
        "db_type": engine,
        "user": user,
        "password": password,
        "encryption_key": encryption_key,
    });

    let result = agent.post("/db-backups/dump", Some(body)).await
        .map_err(|e| agent_error("Database backup", e))?;

    let filename = result.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let size_bytes = result.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;
    let encrypted = encryption_key.is_some();

    // v2.8.2: integrity chain — same pattern as routes/backups.rs for site backups.
    let sha256_hash = result.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let previous_hash: Option<String> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT sha256_hash FROM database_backups WHERE database_id = $1 ORDER BY created_at DESC LIMIT 1"
    ).bind(db_id).fetch_optional(&state.db).await.unwrap_or(None).flatten();

    let backup: DatabaseBackup = sqlx::query_as(
        "INSERT INTO database_backups (database_id, server_id, filename, size_bytes, db_type, db_name, encrypted, sha256_hash, previous_hash, chain_valid) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, TRUE) RETURNING *"
    )
    .bind(db_id).bind(server_id).bind(&filename).bind(size_bytes)
    .bind(&engine).bind(&db_name).bind(encrypted)
    .bind(if sha256_hash.is_empty() { None } else { Some(&sha256_hash) })
    .bind(previous_hash.as_deref())
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("create db backup", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "db_backup.create",
        Some("database"), Some(&db_name), Some(&filename), None,
    ).await;

    fire_event(&state.db, "db_backup.created", serde_json::json!({
        "database": &db_name, "filename": &filename, "size_bytes": size_bytes, "encrypted": encrypted,
    }));

    Ok((StatusCode::CREATED, Json(serde_json::json!({
        "id": backup.id,
        "filename": filename,
        "size_bytes": size_bytes,
        "encrypted": encrypted,
    }))))
}

/// GET /api/backup-orchestrator/db-backups — List database backups.
pub async fn list_db_backups(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<Vec<DatabaseBackup>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let backups: Vec<DatabaseBackup> = sqlx::query_as(
        "SELECT db.* FROM database_backups db \
         JOIN databases d ON d.id = db.database_id JOIN sites s ON d.site_id = s.id AND s.user_id = $1 \
         ORDER BY db.created_at DESC LIMIT $2 OFFSET $3"
    )
    .bind(claims.sub).bind(limit).bind(offset)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list db backups", e))?;

    Ok(Json(backups))
}

/// DELETE /api/backup-orchestrator/db-backups/{id} — Delete a database backup.
pub async fn delete_db_backup(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let backup: Option<DatabaseBackup> = sqlx::query_as(
        "SELECT db.* FROM database_backups db \
         JOIN databases d ON d.id = db.database_id JOIN sites s ON d.site_id = s.id AND s.user_id = $1 \
         WHERE db.id = $2"
    )
    .bind(claims.sub).bind(id)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("delete db backup", e))?;

    let backup = backup.ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

    // Validate filename before constructing agent path (prevent path traversal from stored data)
    if backup.filename.contains('/') || backup.filename.contains("..") || backup.filename.contains('\0') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid backup filename"));
    }

    // Unlink the archive on the host that HOLDS it, which is the host its own row
    // names — not the one the caller's server switcher happened to be pointing at.
    // The `SELECT db.*` above already loaded `server_id`; only the dispatch ignored it.
    // Deleting through the caller's scope asked some other machine to unlink a path it
    // never had, and the DELETE below then ran regardless: the real archive survived
    // with its only record destroyed, invisible to the retention pruner that would
    // eventually have reclaimed it. Where the same dump name exists on both hosts —
    // policy-driven dumps are named per database, so it does — the wrong machine's
    // backup was destroyed instead.
    let agent = crate::helpers::agent_for_site_server(
        &state, backup.server_id, &format!("database {}", backup.db_name),
    ).await?;

    // Delete from agent
    let agent_path = format!("/db-backups/{}/{}", backup.db_name, backup.filename);
    agent.delete(&agent_path).await
        .map_err(|e| agent_error("Delete backup", e))?;

    sqlx::query("DELETE FROM database_backups WHERE id = $1")
        .bind(id).execute(&state.db).await
        .map_err(|e| internal_error("delete db backup", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/backup-orchestrator/db-backups/{id}/restore — Restore a database from backup.
pub async fn restore_db_backup(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Look up the backup record, verify ownership
    let backup: Option<DatabaseBackup> = sqlx::query_as(
        "SELECT db.* FROM database_backups db \
         JOIN databases d ON d.id = db.database_id JOIN sites s ON d.site_id = s.id AND s.user_id = $1 \
         WHERE db.id = $2"
    )
    .bind(claims.sub).bind(id)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("restore db backup", e))?;

    let backup = backup.ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

    // One host governs BOTH directions here, and it is the archive's: the agent reads
    // the dump off its own disk and pipes it into its own `dockpanel-db-{name}`
    // container, so source and destination are the same machine by construction. That
    // is precisely why taking it from the caller's scope was destructive rather than
    // merely wrong — a container of that name answers on every host that runs this
    // database, and the name is unique only per site. The caller's machine would find
    // a container, find a dump beside it, and overwrite a live database the operator
    // was not even looking at, then report success. Pinning to `backup.server_id`
    // instead means a restore either lands on the machine the dump came from or is
    // refused; if that host has since stopped running the container, the agent fails
    // loudly, which is the direction that can be recovered from.
    let agent = crate::helpers::agent_for_site_server(
        &state, backup.server_id, &format!("database {}", backup.db_name),
    ).await?;

    // Fetch database credentials (join with sites for user/password)
    let creds: Option<(String, String, String)> = sqlx::query_as(
        "SELECT d.engine, d.db_user, d.db_password_enc FROM databases d WHERE d.id = $1"
    )
    .bind(backup.database_id)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("restore db backup", e))?;

    let (_engine, user, password_enc) =
        creds.ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;

    // Decrypt the database password (handles both encrypted and legacy plaintext)
    let password = crate::services::secrets_crypto::decrypt_credential_or_legacy(&password_enc, &state.config.jwt_secret);

    let container_name = format!("dockpanel-db-{}", backup.db_name);

    // Get encryption key if backup is encrypted. Policy-encrypted DB backups are encrypted by
    // backup_policy_executor with a key derived from the process JWT secret; the restore MUST
    // use the SAME derivation. The previous code read backup_destinations.encryption_key — a
    // column nothing ever writes — so every encrypted-backup restore 400'd and DR was silently
    // broken in encrypted mode (lesson #70).
    //
    // ⚠ s434: uses the row's OWN `encryption_key_version`, not
    // `derive_backup_encryption_key` (== always-current). The derivation
    // changed once already (a leaky v1 → a safe v2, see
    // `secrets_crypto::derive_backup_encryption_key_v2`'s doc), and a backup
    // written under v1 stays restorable only if restore reproduces v1 for
    // THAT row specifically — reading `CURRENT_BACKUP_KEY_VERSION` here would
    // silently derive the WRONG passphrase for every backup older than
    // whatever the derivation happens to be today.
    let encryption_key: Option<String> = if backup.encrypted {
        Some(crate::services::backup_policy_executor::derive_backup_encryption_key_for_version(
            backup.encryption_key_version,
            &state.config.jwt_secret,
        ))
    } else {
        None
    };

    // Call agent to restore database
    let body = serde_json::json!({
        "container_name": container_name,
        "db_type": backup.db_type,
        "user": user,
        "password": password,
        "encryption_key": encryption_key,
    });

    // Defense-in-depth (parity with delete_db_backup, which guards this): reject any
    // traversal in the stored db_name/filename before composing the agent path. Both
    // come from server-generated rows today, but keep restore and delete symmetric.
    if backup.filename.contains('/') || backup.filename.contains("..") || backup.filename.contains('\0')
        || backup.db_name.contains('/') || backup.db_name.contains("..") || backup.db_name.contains('\0') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid backup path"));
    }

    let agent_path = format!("/db-backups/{}/restore/{}", backup.db_name, backup.filename);
    // `post` and not `post_long` was a 60-second budget on an operation the agent gives
    // 600 (agent/src/services/database_backup.rs), on both transports — the local client
    // times out at 60s and the remote reqwest client defaults to the same. Any restore of
    // a database bigger than a toy therefore reported `agent request timed out after 60s`,
    // which `agent_error` turns into a bare incident id, WHILE THE RESTORE CARRIED ON
    // SUCCEEDING inside the container. The operator was told their restore failed, on a
    // path whose whole purpose is disaster recovery.
    let result = agent
        .post_long(&agent_path, Some(body), crate::services::agent::DB_RESTORE_TIMEOUT_SECS)
        .await
        .map_err(|e| agent_error("Database restore", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "db_backup.restore",
        Some("database"), Some(&backup.db_name), Some(&backup.filename), None,
    ).await;

    fire_event(&state.db, "db_backup.restored", serde_json::json!({
        "database": &backup.db_name, "filename": &backup.filename, "backup_id": id.to_string(),
    }));

    Ok(Json(result))
}

// ── Volume Backups ──────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct CreateVolumeBackupRequest {
    pub container_id: String,
    pub container_name: String,
    pub volume_name: String,
}

/// POST /api/backup-orchestrator/volume-backup — Create a volume backup.
pub async fn create_volume_backup(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(scope_server_id, agent): ServerScope,
    Json(req): Json<CreateVolumeBackupRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // Validate container/volume names to prevent path traversal in agent URLs
    if req.container_name.contains('/') || req.container_name.contains("..") || req.container_name.contains('\0') || req.container_name.len() > 128 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container name"));
    }
    if req.volume_name.contains('/') || req.volume_name.contains("..") || req.volume_name.contains('\0') || req.volume_name.len() > 128 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid volume name"));
    }

    let body = serde_json::json!({
        "volume_name": req.volume_name,
        "container_name": req.container_name,
    });

    let result = agent.post("/volume-backups/create", Some(body)).await
        .map_err(|e| agent_error("Volume backup", e))?;

    let filename = result.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let size_bytes = result.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;

    // The row records the server the archive was actually written on, which is the
    // one `ServerScope` just resolved the agent for — NOT "some online server".
    // Picking an arbitrary row here labelled the backup with a host that never held
    // it, and two background services read this column to decide where to act: the
    // verifier looks for the file there, and the drill scheduler restores from it.
    let server_id: Option<Uuid> = Some(scope_server_id);

    // v2.8.2: integrity chain. Scope previous_hash by (container_id, volume_name)
    // so re-creating the same logical volume re-chains rather than forking.
    let sha256_hash = result.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let previous_hash: Option<String> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT sha256_hash FROM volume_backups WHERE container_id = $1 AND volume_name = $2 ORDER BY created_at DESC LIMIT 1"
    ).bind(&req.container_id).bind(&req.volume_name).fetch_optional(&state.db).await.unwrap_or(None).flatten();

    let backup: VolumeBackup = sqlx::query_as(
        "INSERT INTO volume_backups (container_id, container_name, server_id, volume_name, filename, size_bytes, sha256_hash, previous_hash, chain_valid) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, TRUE) RETURNING *"
    )
    .bind(&req.container_id).bind(&req.container_name).bind(server_id)
    .bind(&req.volume_name).bind(&filename).bind(size_bytes)
    .bind(if sha256_hash.is_empty() { None } else { Some(&sha256_hash) })
    .bind(previous_hash.as_deref())
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("create volume backup", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "volume_backup.create",
        Some("volume"), Some(&req.container_name), Some(&filename), None,
    ).await;

    Ok((StatusCode::CREATED, Json(serde_json::json!({
        "id": backup.id,
        "filename": filename,
        "size_bytes": size_bytes,
    }))))
}

/// GET /api/backup-orchestrator/volume-backups — List volume backups.
pub async fn list_volume_backups(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<Vec<VolumeBackup>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    // Scoped the same way `restore_volume_backup` below scopes its own lookup:
    // this machine, or a server this operator added. Before this fix the query
    // was a flat, unfiltered `SELECT *` — every admin account's volume-backup
    // metadata (container/volume names, filenames, sizes, hashes, server_id)
    // was visible to every OTHER admin account, unlike the sibling
    // `list_db_backups` above, which has always scoped by `s.user_id`. A row
    // whose `server_id` predates the fleet backfill (NULL) is excluded by the
    // JOIN rather than shown to nobody-in-particular, matching how the restore
    // path below refuses it outright.
    let backups: Vec<VolumeBackup> = sqlx::query_as(
        "SELECT vb.* FROM volume_backups vb \
         JOIN servers sv ON sv.id = vb.server_id AND (sv.is_local OR sv.user_id = $1) \
         ORDER BY vb.created_at DESC LIMIT $2 OFFSET $3"
    )
    .bind(claims.sub).bind(limit).bind(offset)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list volume backups", e))?;

    Ok(Json(backups))
}

/// POST /api/backup-orchestrator/volume-backups/{id}/restore — Restore a volume from backup.
pub async fn restore_volume_backup(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Look up the volume backup record, scoped to the machines this operator runs.
    //
    // The scope has to land in the SAME commit as the body fix below, and that
    // ordering is the whole point. Until now this route could not succeed at all —
    // it sent no request body, so the agent's `Json` extractor answered 415 before
    // any handler ran — and a route that always fails closed hides whatever else is
    // wrong with it. Fixing only the body would have made an unscoped restore work
    // for the first time: `volume_backups` has no owner column, only `server_id`,
    // and the agent is resolved from that row, so any administrator could unpack a
    // backup over a live volume on a server somebody else registered.
    //
    // The predicate is the one `SITE_CALLER_PREDICATE`'s admin arm draws and
    // `approve_deploy` reuses — this machine, or a server this operator added — and
    // `u.role` is read from the DATABASE, not from `claims.role`, because a JWT
    // keeps asserting the role it was minted with until it expires.
    //
    // A NULL `server_id` (a row predating the fleet backfill) no longer reaches the
    // 409 below: it cannot satisfy the scope, so it 404s here. Refusing it earlier
    // is the same answer for the same reason — there is no safe guess about which
    // machine's volume to overwrite — and it also stops the id of a backup on
    // another operator's server from being distinguishable from one that does not.
    let backup: Option<VolumeBackup> = sqlx::query_as(
        "SELECT * FROM volume_backups vb WHERE vb.id = $1 AND EXISTS (\
             SELECT 1 FROM users u, servers sv WHERE u.id = $2 AND u.role = 'admin' \
             AND sv.id = vb.server_id AND (sv.is_local OR sv.user_id = u.id))"
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("restore volume backup", e))?;

    let backup = backup.ok_or_else(|| err(StatusCode::NOT_FOUND, "Volume backup not found"))?;

    // Source and destination are the same host for a volume restore: the agent unpacks
    // a tar from its own disk straight over its own Docker volume. So `server_id`
    // governs both ends, and `create_volume_backup` above deliberately records the host
    // that actually wrote the archive for exactly this reader. Restoring through the
    // caller's scope did NOT merely fail to find the file — policy-driven volume
    // archives are named per container and volume, so an identically-named tar sits on
    // every host running that container, and the caller's machine would happily unpack
    // ITS OWN archive over ITS OWN live volume: a restore nobody requested, on a server
    // the operator was not viewing, reported as the success of a different one. A NULL
    // here is a row written before the fleet backfill; refusing it is correct, because
    // there is no safe guess about which machine's volume to overwrite.
    let agent = crate::helpers::agent_for_site_server(
        &state, backup.server_id, &format!("volume {}", backup.volume_name),
    ).await?;

    // Call agent to restore volume.
    //
    // The body is not optional and never was. `restore` on the agent side takes
    // `Json<VolumeRestoreRequest>`, whose single field `volume_name` is what tells
    // it which volume to unpack over; a required `Json` extractor rejects a request
    // carrying no `content-type` with 415 before the handler runs, and neither
    // agent transport sets that header when the body is `None`
    // (`services/agent.rs` sets it only `if body.is_some()`, and the remote twin
    // only calls `.json()` inside `if let Some(b)`). So every call this route has
    // ever made was refused in the transport layer — the sibling `restore_db_backup`
    // passes `Some(body)` and worked, which is the asymmetry that hid it.
    //
    // The volume drill cannot catch this: `services/backup_drill.rs` re-implements
    // the restore rather than calling this path, so a green drill says nothing
    // about whether the real restore runs.
    let agent_path = format!("/volume-backups/{}/restore/{}", backup.container_name, backup.filename);
    let result = agent.post(
        &agent_path,
        Some(serde_json::json!({ "volume_name": backup.volume_name })),
    ).await
        .map_err(|e| agent_error("Volume restore", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "volume_backup.restore",
        Some("volume"), Some(&backup.container_name), Some(&backup.filename), None,
    ).await;

    fire_event(&state.db, "volume_backup.restored", serde_json::json!({
        "container_name": &backup.container_name, "volume_name": &backup.volume_name,
        "filename": &backup.filename, "backup_id": id.to_string(),
    }));

    Ok(Json(result))
}

// ── Verification ────────────────────────────────────────────────────────────

/// Resolve the host a stored backup actually lives on, from the backup's own row.
///
/// Verify and drill are the two endpoints that take nothing but `(backup_type,
/// backup_id)` — no site, no database, no container — so there was nothing in the
/// request to pin a host to and both simply used whatever agent `ServerScope` handed
/// over. That reads one machine's disk looking for another machine's archive. For a
/// verification the result is a `failed` row that says more about the caller's server
/// switcher than about the backup, which then feeds the SLA and stale-backup panels as
/// though the archive were bad. For a drill it is worse: a drill RESTORES.
///
/// The lookup is a deliberate extra round-trip taken BEFORE the tracking row is
/// inserted — the same ordering `services/backup_verifier.rs::verify_one` spells out and
/// for the same reason: a row written `running` against a host we then decline to reach
/// is a permanent record of work that never happened, and the sweeper's driving queries
/// skip any backup that already has one.
///
/// Returns the server id beside the handle so the caller can stamp its own row with the
/// host that did the work. It stays `Option` because these columns are nullable for rows
/// predating the fleet backfill; by the time this returns, `agent_for_site_server` has
/// already refused the `None` case, so what gets bound is always `Some`.
async fn agent_for_backup(
    state: &AppState,
    backup_type: &str,
    backup_id: Uuid,
) -> Result<(Option<Uuid>, crate::services::agent::AgentHandle), ApiError> {
    // `backups` carries no server_id of its own — a site backup reaches its host through
    // the site, the same hop the unified list and the SLA queries above already make.
    let sql = match backup_type {
        "site" => "SELECT s.server_id FROM backups b JOIN sites s ON s.id = b.site_id WHERE b.id = $1",
        "database" => "SELECT server_id FROM database_backups WHERE id = $1",
        "volume" => "SELECT server_id FROM volume_backups WHERE id = $1",
        _ => return Err(err(StatusCode::BAD_REQUEST, "Invalid backup type")),
    };

    let server_id: Option<Uuid> = sqlx::query_scalar::<_, Option<Uuid>>(sql)
        .bind(backup_id)
        .fetch_optional(&state.db).await
        .map_err(|e| internal_error("resolve backup host", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

    let agent = crate::helpers::agent_for_site_server(
        state, server_id, &format!("{backup_type} backup {backup_id}"),
    ).await?;
    Ok((server_id, agent))
}

#[derive(serde::Deserialize)]
pub struct VerifyRequest {
    pub backup_type: String, // site, database, volume
    pub backup_id: Uuid,
}

/// POST /api/backup-orchestrator/verify — Trigger backup verification.
pub async fn trigger_verify(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Json(req): Json<VerifyRequest>,
) -> Result<(StatusCode, Json<BackupVerification>), ApiError> {
    // Read the archive on the host that holds it, resolved from the backup's own row.
    // The agent used to come from `ServerScope` — and the row below was then stamped
    // with that same scope, so the record and the work agreed with each other while
    // both pointed at a machine chosen by a request header. `backup_verifier.rs`, the
    // scheduled half of this very feature, has always resolved `for_server(server_id)`
    // from the row; only the operator-pressed button kept reading the switcher.
    //
    // This also moves the unsupported-type rejection to a synchronous 400. It used to
    // be an `Invalid backup type` string produced inside the spawned task, i.e. a 202
    // followed by a `failed` verification row for a request that was never valid.
    let (server_id, agent) = agent_for_backup(&state, &req.backup_type, req.backup_id).await?;

    // Create pending verification record. `server_id` is bound rather than left
    // NULL: it names the host that actually read the archive. Leaving it NULL is what
    // made this column dead in every writer while the UI still rendered a Server
    // field for it.
    let verification: BackupVerification = sqlx::query_as(
        "INSERT INTO backup_verifications (backup_type, backup_id, server_id, status, started_at) \
         VALUES ($1, $2, $3, 'running', NOW()) RETURNING *"
    )
    .bind(&req.backup_type).bind(req.backup_id).bind(server_id)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("trigger verify", e))?;

    let verif_id = verification.id;
    let db = state.db.clone();
    let backup_type = req.backup_type.clone();
    let backup_id = req.backup_id;

    // Run verification async
    tokio::spawn(async move {
        let result: Result<serde_json::Value, String> = match backup_type.as_str() {
            "site" => {
                let row = sqlx::query_as::<_, (String, String)>(
                    "SELECT s.domain, b.filename FROM backups b JOIN sites s ON s.id = b.site_id WHERE b.id = $1"
                ).bind(backup_id).fetch_optional(&db).await;

                match row {
                    Ok(Some((domain, filename))) => {
                        let body = serde_json::json!({ "domain": domain, "filename": filename });
                        agent.post("/backups/verify/site", Some(body)).await.map_err(|e| e.to_string())
                    }
                    Ok(None) => Err("Backup not found".to_string()),
                    Err(e) => {
                        tracing::warn!("DB error fetching site backup for verification: {e}");
                        Err(format!("Database error: {e}"))
                    }
                }
            }
            "database" => {
                let row = sqlx::query_as::<_, (String, String, String)>(
                    "SELECT db_type, db_name, filename FROM database_backups WHERE id = $1"
                ).bind(backup_id).fetch_optional(&db).await;

                match row {
                    Ok(Some((db_type, db_name, filename))) => {
                        let body = serde_json::json!({ "db_type": db_type, "db_name": db_name, "filename": filename });
                        agent.post("/backups/verify/database", Some(body)).await.map_err(|e| e.to_string())
                    }
                    Ok(None) => Err("Database backup not found".to_string()),
                    Err(e) => {
                        tracing::warn!("DB error fetching database backup for verification: {e}");
                        Err(format!("Database error: {e}"))
                    }
                }
            }
            "volume" => {
                let row = sqlx::query_as::<_, (String, String)>(
                    "SELECT container_name, filename FROM volume_backups WHERE id = $1"
                ).bind(backup_id).fetch_optional(&db).await;

                match row {
                    Ok(Some((container_name, filename))) => {
                        let body = serde_json::json!({ "container_name": container_name, "filename": filename });
                        agent.post("/backups/verify/volume", Some(body)).await.map_err(|e| e.to_string())
                    }
                    Ok(None) => Err("Volume backup not found".to_string()),
                    Err(e) => {
                        tracing::warn!("DB error fetching volume backup for verification: {e}");
                        Err(format!("Database error: {e}"))
                    }
                }
            }
            _ => Err("Invalid backup type".to_string()),
        };

        match result {
            Ok(data) => {
                let passed = data.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
                let checks_run = data.get("checks_run").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let checks_passed = data.get("checks_passed").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let duration_ms = data.get("duration_ms").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let details = data.get("details").cloned().unwrap_or(serde_json::json!([]));

                let _ = sqlx::query(
                    "UPDATE backup_verifications SET \
                     status = $2, checks_run = $3, checks_passed = $4, \
                     details = $5, duration_ms = $6, completed_at = NOW() \
                     WHERE id = $1"
                )
                .bind(verif_id)
                .bind(if passed { "passed" } else { "failed" })
                .bind(checks_run).bind(checks_passed)
                .bind(details).bind(duration_ms)
                .execute(&db).await;
            }
            Err(e) => {
                let _ = sqlx::query(
                    "UPDATE backup_verifications SET status = 'failed', error_message = $2, completed_at = NOW() WHERE id = $1"
                ).bind(verif_id).bind(&e).execute(&db).await;
            }
        }
    });

    Ok((StatusCode::ACCEPTED, Json(verification)))
}

/// GET /api/backup-orchestrator/storage-history — Backup storage growth over time (last 30 days).
pub async fn storage_history(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    // Query system_logs for 'backup_storage' entries, aggregate daily totals over last 30 days
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT DATE(created_at)::TEXT as day, message \
         FROM system_logs \
         WHERE source = 'backup_storage' AND created_at > NOW() - INTERVAL '30 days' \
         ORDER BY created_at ASC"
    )
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("storage history", e))?;

    // Group by day, keep the last reading per day
    let mut daily: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for (day, message) in &rows {
        if let Ok(bytes) = message.parse::<i64>() {
            daily.insert(day.clone(), bytes);
        }
    }

    let result: Vec<serde_json::Value> = daily.into_iter()
        .map(|(day, bytes)| serde_json::json!({
            "date": day,
            "total_bytes": bytes,
            "total_mb": (bytes as f64 / 1_048_576.0).round() as i64,
        }))
        .collect();

    Ok(Json(result))
}

/// GET /api/backup-orchestrator/verifications — List verifications.
pub async fn list_verifications(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<Vec<BackupVerification>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let verifications: Vec<BackupVerification> = sqlx::query_as(
        "SELECT * FROM backup_verifications ORDER BY created_at DESC LIMIT $1 OFFSET $2"
    )
    .bind(limit).bind(offset)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list verifications", e))?;

    Ok(Json(verifications))
}

// ── Drills (Phase 4 W1.2: end-to-end restore probes) ────────────────────────

#[derive(serde::Deserialize)]
pub struct DrillRequest {
    pub backup_type: String, // site | database | volume
    pub backup_id: Uuid,
}

/// POST /api/backup-orchestrator/drill — Trigger an on-demand backup drill.
/// Supports `backup_type = "site"` (W1.2.a), `"database"` (W1.2.b),
/// and `"volume"` (W1.2.c).
pub async fn trigger_drill(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(req): Json<DrillRequest>,
) -> Result<(StatusCode, Json<BackupDrill>), ApiError> {
    if req.backup_type != "site" && req.backup_type != "database" && req.backup_type != "volume" {
        return Err(err(StatusCode::BAD_REQUEST, "Unsupported backup_type"));
    }

    // A drill is a real restore, so the host must come from the backup's row and
    // nowhere else. Driven by `ServerScope` this reached for one machine's archive on
    // another: with site backups it merely failed, but policy-driven database and volume
    // archives are named deterministically per resource, so an identically-named file
    // sits on every host running that resource — and the caller's machine would find it,
    // restore ITS OWN archive over ITS OWN live data, and report the result under the
    // server the operator had selected. `drill_scheduler.rs::dispatch_policy_drills`
    // resolves `for_server(row.server_id)` and refuses rather than substituting, with the
    // comment that "a drill that runs elsewhere certifies disaster recovery for a machine
    // it never tested". The scheduled path had it right; this button never adopted it.
    let (server_id, agent) = agent_for_backup(&state, &req.backup_type, req.backup_id).await?;

    // Insert pending drill record so the UI gets immediate feedback. `server_id` is
    // bound now that one is known: `drill_scheduler`'s concurrency guard counts running
    // drills BY SERVER, and a NULL from this handler was invisible to it — an
    // operator-pressed drill could not hold off the scheduled one, so two restores could
    // land on the same host at once. It also fills the Server column the drills table
    // renders, which was blank for every manually triggered row.
    let drill: BackupDrill = sqlx::query_as(
        "INSERT INTO backup_drills (backup_type, backup_id, server_id, triggered_by, status, started_at) \
         VALUES ($1, $2, $3, $4, 'running', NOW()) RETURNING *"
    )
    .bind(&req.backup_type).bind(req.backup_id).bind(server_id).bind(claims.sub)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("trigger drill", e))?;

    let drill_id = drill.id;
    let db = state.db.clone();
    let backup_type = req.backup_type.clone();
    let backup_id = req.backup_id;

    // Run drill async. Uses post_long: a plain `post` caps the whole round trip at 60s,
    // but the agent-side restore this triggers is budgeted 220s (database) to 360s
    // (volume: 300s restore + 60s probe) — see drill_scheduler.rs's identical fix for the
    // scheduled path, which this on-demand button shares the same underlying agent
    // handlers with.
    tokio::spawn(async move {
        let result: Result<serde_json::Value, String> = match backup_type.as_str() {
            "site" => {
                let row = sqlx::query_as::<_, (String, String)>(
                    "SELECT s.domain, b.filename FROM backups b \
                     JOIN sites s ON s.id = b.site_id WHERE b.id = $1"
                ).bind(backup_id).fetch_optional(&db).await;

                match row {
                    Ok(Some((domain, filename))) => {
                        let body = serde_json::json!({ "domain": domain, "filename": filename });
                        agent.post_long("/backups/drill/site", Some(body), crate::services::agent::DRILL_TIMEOUT_SECS).await.map_err(|e| e.to_string())
                    }
                    Ok(None) => Err("Site backup not found".to_string()),
                    Err(e) => {
                        tracing::warn!("DB error fetching site backup for drill: {e}");
                        Err(format!("Database error: {e}"))
                    }
                }
            }
            "database" => {
                let row = sqlx::query_as::<_, (String, String, String)>(
                    "SELECT db_type, db_name, filename FROM database_backups WHERE id = $1"
                ).bind(backup_id).fetch_optional(&db).await;

                match row {
                    Ok(Some((db_type, db_name, filename))) => {
                        let body = serde_json::json!({
                            "db_type": db_type,
                            "db_name": db_name,
                            "filename": filename,
                        });
                        agent.post_long("/backups/drill/db", Some(body), crate::services::agent::DRILL_TIMEOUT_SECS).await.map_err(|e| e.to_string())
                    }
                    Ok(None) => Err("Database backup not found".to_string()),
                    Err(e) => {
                        tracing::warn!("DB error fetching database backup for drill: {e}");
                        Err(format!("Database error: {e}"))
                    }
                }
            }
            "volume" => {
                let row = sqlx::query_as::<_, (String, String)>(
                    "SELECT container_name, filename FROM volume_backups WHERE id = $1"
                ).bind(backup_id).fetch_optional(&db).await;

                match row {
                    Ok(Some((container_name, filename))) => {
                        let body = serde_json::json!({
                            "container_name": container_name,
                            "filename": filename,
                        });
                        agent.post_long("/backups/drill/volume", Some(body), crate::services::agent::DRILL_TIMEOUT_SECS).await.map_err(|e| e.to_string())
                    }
                    Ok(None) => Err("Volume backup not found".to_string()),
                    Err(e) => {
                        tracing::warn!("DB error fetching volume backup for drill: {e}");
                        Err(format!("Database error: {e}"))
                    }
                }
            }
            _ => Err("Unsupported backup type".to_string()),
        };

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

    Ok((StatusCode::ACCEPTED, Json(drill)))
}

#[derive(serde::Serialize)]
pub struct DrillsResponse {
    pub items: Vec<BackupDrill>,
    pub total: i64,
}

/// GET /api/backup-orchestrator/drills — List drills (paginated).
pub async fn list_drills(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<DrillsResponse>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let items: Vec<BackupDrill> = sqlx::query_as(
        "SELECT * FROM backup_drills ORDER BY created_at DESC LIMIT $1 OFFSET $2"
    )
    .bind(limit).bind(offset)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list drills", e))?;

    let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_drills")
        .fetch_one(&state.db).await
        .map_err(|e| internal_error("list drills count", e))?;

    Ok(Json(DrillsResponse { items, total }))
}

// ── Chain-of-Trust Report (Phase 4 W1.3) ────────────────────────────────────
//
// v2.8.1 shipped site-only. v2.8.2 extends to db + volume after migration
// 20260430200000 added `sha256_hash`/`previous_hash`/`chain_valid` to those
// tables, and the agent now computes SHA-256 during db_backup + volume_backup.
// Single `build_chain_report(kind, id)` dispatches on table; the typst template
// branches on `backup.kind` to render the right resource label.

#[derive(serde::Serialize)]
pub struct ChainReportBackup {
    /// One of "site" | "database" | "volume". The typst template branches on this.
    pub kind: String,
    pub id: Uuid,
    /// Domain for site, db_name for database, "container:volume" for volume.
    pub resource_name: String,
    pub filename: String,
    pub size_bytes: i64,
    pub sha256_hash: Option<String>,
    pub previous_hash: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    // Kind-specific extras — exactly one set is populated, the others are None.
    pub site_id: Option<Uuid>,
    pub database_id: Option<Uuid>,
    pub container_id: Option<String>,
    pub volume_name: Option<String>,
    pub db_type: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ChainReportVerification {
    pub id: Uuid,
    pub status: String,
    pub checks_run: i32,
    pub checks_passed: i32,
    pub duration_ms: Option<i32>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub error_message: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ChainReportDrill {
    pub id: Uuid,
    pub status: String,
    pub http_status: Option<i32>,
    pub body_excerpt: Option<String>,
    pub duration_ms: Option<i32>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub error_message: Option<String>,
}

// No validity verdict here, deliberately. `chain_valid` is written TRUE by every
// one of its seven INSERTs — five literal, two by column default — is UPDATEd
// nowhere, and no verifier exists that could ever compute it: `previous_hash` is
// copied out of the newest row rather than recomputed from the predecessor
// artifact. A verdict this document cannot evaluate has no business being
// rendered in it, so the report carries what the panel genuinely recorded and
// stops there. Re-adding a verdict field means building the verifier first.
#[derive(serde::Serialize)]
pub struct ChainIntegrity {
    pub verifications_passed: i64,
    pub drills_passed: i64,
}

#[derive(serde::Serialize)]
pub struct ChainReport {
    pub panel_version: String,
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub backup: ChainReportBackup,
    pub verifications: Vec<ChainReportVerification>,
    pub drills: Vec<ChainReportDrill>,
    pub chain_integrity: ChainIntegrity,
}

/// Validate the {kind} URL segment. Returns the canonical lowercase value or
/// 400 if the kind is unknown. Kept centralised so the JSON + PDF handlers
/// don't drift.
fn parse_chain_kind(kind: &str) -> Result<&'static str, ApiError> {
    match kind {
        "site" => Ok("site"),
        "database" => Ok("database"),
        "volume" => Ok("volume"),
        _ => Err(err(StatusCode::BAD_REQUEST, "kind must be one of: site, database, volume")),
    }
}

/// Build a chain-of-trust report for any backup kind. Returns 404 if the
/// backup doesn't exist. Single point of truth for the JSON + PDF endpoints.
///
/// v2.8.2: refactored from build_site_chain_report. Backup-row shape varies
/// per kind so each branch issues its own SELECT; the verifications/drills
/// queries are uniform — they just take `backup_type` as a parameter.
async fn build_chain_report(
    state: &AppState,
    kind: &'static str,
    backup_id: Uuid,
    caller_id: Uuid,
) -> Result<ChainReport, ApiError> {
    // Every branch below joins `servers` and scopes by `sv.is_local OR sv.user_id =
    // caller_id` — the same predicate `list_volume_backups`/`restore_volume_backup`
    // already established for this exact fleet (v2.184.0). Before this fix `AdminUser`
    // alone gated these two handlers, and every query here was a bare `WHERE id = $1`:
    // any admin could pull another admin's backup provenance (site domain, db/container
    // names, filenames, full hash chain, every verification and drill) as JSON or PDF —
    // the same bug `list_all_backups` below carries, just reached through a different
    // route. A `server_id` predating the fleet backfill (NULL, database/volume only —
    // `sites.server_id` is NOT NULL) fails closed via the JOIN rather than falling back
    // to "visible to nobody in particular", matching the established precedent.
    let backup = match kind {
        "site" => {
            let row: Option<(
                Uuid, Uuid, String, String, i64,
                Option<String>, Option<String>,
                chrono::DateTime<chrono::Utc>,
            )> = sqlx::query_as(
                "SELECT b.id, b.site_id, s.domain, b.filename, b.size_bytes, \
                        b.sha256_hash, b.previous_hash, b.created_at \
                   FROM backups b JOIN sites s ON s.id = b.site_id \
                   JOIN servers sv ON sv.id = s.server_id AND (sv.is_local OR sv.user_id = $2) \
                  WHERE b.id = $1"
            )
            .bind(backup_id)
            .bind(caller_id)
            .fetch_optional(&state.db).await
            .map_err(|e| internal_error("chain report: load site backup", e))?;

            let (id, site_id, resource_name, filename, size_bytes, sha256_hash, previous_hash, created_at) =
                row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

            ChainReportBackup {
                kind: "site".into(),
                id, resource_name, filename, size_bytes,
                sha256_hash, previous_hash,
                created_at,
                site_id: Some(site_id),
                database_id: None, container_id: None, volume_name: None, db_type: None,
            }
        }
        "database" => {
            let row: Option<(
                Uuid, Uuid, String, String, String, i64,
                Option<String>, Option<String>,
                chrono::DateTime<chrono::Utc>,
            )> = sqlx::query_as(
                "SELECT db.id, db.database_id, db.db_name, db.db_type, db.filename, db.size_bytes, \
                        db.sha256_hash, db.previous_hash, db.created_at \
                   FROM database_backups db \
                   JOIN servers sv ON sv.id = db.server_id AND (sv.is_local OR sv.user_id = $2) \
                  WHERE db.id = $1"
            )
            .bind(backup_id)
            .bind(caller_id)
            .fetch_optional(&state.db).await
            .map_err(|e| internal_error("chain report: load database backup", e))?;

            let (id, database_id, resource_name, db_type, filename, size_bytes, sha256_hash, previous_hash, created_at) =
                row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

            ChainReportBackup {
                kind: "database".into(),
                id, resource_name, filename, size_bytes,
                sha256_hash, previous_hash,
                created_at,
                site_id: None,
                database_id: Some(database_id),
                container_id: None, volume_name: None,
                db_type: Some(db_type),
            }
        }
        "volume" => {
            let row: Option<(
                Uuid, String, String, String, String, i64,
                Option<String>, Option<String>,
                chrono::DateTime<chrono::Utc>,
            )> = sqlx::query_as(
                "SELECT vb.id, vb.container_id, vb.container_name, vb.volume_name, vb.filename, vb.size_bytes, \
                        vb.sha256_hash, vb.previous_hash, vb.created_at \
                   FROM volume_backups vb \
                   JOIN servers sv ON sv.id = vb.server_id AND (sv.is_local OR sv.user_id = $2) \
                  WHERE vb.id = $1"
            )
            .bind(backup_id)
            .bind(caller_id)
            .fetch_optional(&state.db).await
            .map_err(|e| internal_error("chain report: load volume backup", e))?;

            let (id, container_id, container_name, volume_name, filename, size_bytes, sha256_hash, previous_hash, created_at) =
                row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

            ChainReportBackup {
                kind: "volume".into(),
                id,
                resource_name: format!("{container_name}:{volume_name}"),
                filename, size_bytes,
                sha256_hash, previous_hash,
                created_at,
                site_id: None, database_id: None,
                container_id: Some(container_id),
                volume_name: Some(volume_name),
                db_type: None,
            }
        }
        // parse_chain_kind already gates this — defensive only.
        _ => return Err(err(StatusCode::BAD_REQUEST, "Unsupported backup kind")),
    };

    let verifications_rows: Vec<(
        Uuid, String, i32, i32, Option<i32>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
        chrono::DateTime<chrono::Utc>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT id, status, checks_run, checks_passed, duration_ms, \
                started_at, completed_at, created_at, error_message \
           FROM backup_verifications \
          WHERE backup_type = $1 AND backup_id = $2 \
          ORDER BY created_at ASC"
    )
    .bind(kind)
    .bind(backup_id)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("chain report: load verifications", e))?;

    let drills_rows: Vec<(
        Uuid, String, Option<i32>, Option<String>, Option<i32>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
        chrono::DateTime<chrono::Utc>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT id, status, http_status, body_excerpt, duration_ms, \
                started_at, completed_at, created_at, error_message \
           FROM backup_drills \
          WHERE backup_type = $1 AND backup_id = $2 \
          ORDER BY created_at ASC"
    )
    .bind(kind)
    .bind(backup_id)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("chain report: load drills", e))?;

    let verifications: Vec<ChainReportVerification> = verifications_rows.into_iter().map(
        |(id, status, checks_run, checks_passed, duration_ms, started_at, completed_at, created_at, error_message)| {
            ChainReportVerification {
                id, status, checks_run, checks_passed, duration_ms,
                started_at, completed_at, created_at, error_message,
            }
        }
    ).collect();

    let drills: Vec<ChainReportDrill> = drills_rows.into_iter().map(
        |(id, status, http_status, body_excerpt, duration_ms, started_at, completed_at, created_at, error_message)| {
            ChainReportDrill {
                id, status, http_status, body_excerpt, duration_ms,
                started_at, completed_at, created_at, error_message,
            }
        }
    ).collect();

    let verifications_passed = verifications.iter().filter(|v| v.status == "passed").count() as i64;
    let drills_passed = drills.iter().filter(|d| d.status == "passed").count() as i64;
    Ok(ChainReport {
        panel_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: chrono::Utc::now(),
        backup,
        verifications,
        drills,
        chain_integrity: ChainIntegrity {
            verifications_passed,
            drills_passed,
        },
    })
}

/// Helper: short backup id suffix for filename. e.g. "a1b2c3d4".
fn short_id(id: Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

/// Helper: sanitise a site name into a filesystem-safe slug for the PDF
/// filename. Keeps alphanum + dashes; collapses other chars to "-".
fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// GET /api/backup-orchestrator/chain-report/{kind}/{id} — Chain-of-trust
/// JSON for one backup of any kind (site | database | volume). Single
/// artifact, full provenance chain (backup integrity hashes + every
/// verification + every restore drill).
pub async fn chain_report_json(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path((kind, id)): Path<(String, Uuid)>,
) -> Result<Json<ChainReport>, ApiError> {
    let kind = parse_chain_kind(&kind)?;
    let report = build_chain_report(&state, kind, id, claims.sub).await?;
    Ok(Json(report))
}

/// GET /api/backup-orchestrator/chain-report/{kind}/{id}/pdf — Chain-of-trust
/// PDF rendered via typst. First call lazy-installs typst into
/// /var/lib/dockpanel/typst (~30MB, one-time). 503 if install/compile fails.
pub async fn chain_report_pdf(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path((kind, id)): Path<(String, Uuid)>,
) -> Result<axum::response::Response, ApiError> {
    let kind = parse_chain_kind(&kind)?;
    let report = build_chain_report(&state, kind, id, claims.sub).await?;
    let json_value = serde_json::to_value(&report)
        .map_err(|e| internal_error("chain report: serialize", e))?;

    let pdf = crate::services::chain_report::render_chain_report_pdf(&json_value)
        .await
        .map_err(|e| {
            tracing::warn!("chain report pdf render failed: {e}");
            err(StatusCode::SERVICE_UNAVAILABLE, &format!("PDF generation failed: {e}"))
        })?;

    let filename = format!(
        "chain-report-{}-{}-{}.pdf",
        report.backup.kind,
        slug(&report.backup.resource_name),
        short_id(report.backup.id),
    );

    use axum::http::header;
    let response = axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/pdf")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(axum::body::Body::from(pdf))
        .map_err(|e| internal_error("chain report: build response", e))?;

    Ok(response)
}
