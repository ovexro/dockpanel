//! Phase 4 W5: fleet configuration-drift detection.
//!
//! A read-only report answering "is my fleet's operational posture
//! consistent?". DockPanel is a single hub DB + thin agents, so every
//! server's declared config already lives centrally keyed by `server_id`
//! (`20260319000000_multi_server.sql`). Drift is therefore a LOCAL
//! cross-`server_id` diff — no remote agent pull, offline-tolerant, and
//! computed on demand (brief §7 "lean lazy": operator-triggered, no
//! background scan).
//!
//! v1 is REPORT-ONLY. Reconcile/push (source-of-truth → apply to others) is
//! deferred to W5.2 — it is net-new cross-server mutation with no existing
//! transport, and the brief keeps that surface explicit (§W5 "report, not an
//! enforcement loop"; §4 "operability ≠ automation"). Desired-vs-actual drift
//! against the agent's live `GET /iac/export` is deferred to W5.3.
//!
//! Four entities in v1, each compared in its most meaningful form:
//!   - `alert_rules`     — SINGLETON: one row per server, whole-row posture diff (flagship).
//!   - `sites`           — COLLECTION: per-domain inventory asymmetry + per-site config diff.
//!   - `crons`           — COLLECTION: per (domain, command) job parity.
//!   - `backup_coverage` — SUMMARY: per-server derived coverage (sites total / backed-up / destinations).
//!
//! The comparators (`compare_config`, `diff_singleton`, `diff_collection`)
//! are pure and unit-tested with in-memory rows; the fetchers project each
//! entity's config columns to a JSONB object in SQL so the engine diffs a
//! generic `serde_json::Value` object per row.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use std::collections::{BTreeSet, HashMap, HashSet};
use uuid::Uuid;

const NULL: Value = Value::Null;

/// Secret-ish columns compared by presence only — the report never surfaces
/// the value, only "set" vs "unset", so a rotated secret doesn't read as drift
/// and a leaked value never reaches the UI.
const ALERT_SENSITIVE: &[&str] = &["notify_slack_url", "notify_discord_url"];
const NO_SENSITIVE: &[&str] = &[];

// ─── Wire types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ServerRef {
    pub server_id: Uuid,
    pub name: String,
    pub is_local: bool,
    pub status: String,
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FieldDiff {
    pub field: String,
    /// The reference server's value (or the "set"/"unset" token for a sensitive field).
    pub reference: Value,
    /// This server's value (or token).
    pub current: Value,
    pub sensitive: bool,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct EntityValueDiff {
    /// Natural identity of the drifted entity (e.g. a site domain).
    pub identity: String,
    pub fields: Vec<FieldDiff>,
}

#[derive(Serialize, Clone, Copy)]
pub struct CoverageMetrics {
    pub total_sites: i64,
    pub backed_up: i64,
    pub unprotected: i64,
    pub destinations: i64,
}

#[derive(Serialize)]
pub struct ServerEntityDrift {
    pub server_id: Uuid,
    pub name: String,
    pub status: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub in_sync: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    // SINGLETON
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_diffs: Option<Vec<FieldDiff>>,
    // COLLECTION
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only_here: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only_reference: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_diffs: Option<Vec<EntityValueDiff>>,
    // SUMMARY
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<CoverageMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_coverage: Option<CoverageMetrics>,
}

#[derive(Serialize)]
pub struct EntityDrift {
    pub entity: String,
    pub label: String,
    pub mode: String,
    /// Number of compared servers that diverge from the reference for this entity.
    pub drift_count: usize,
    pub servers: Vec<ServerEntityDrift>,
}

#[derive(Serialize)]
pub struct DriftReport {
    pub reference: ServerRef,
    pub generated_at: DateTime<Utc>,
    pub servers_compared: usize,
    pub total_drifted_servers: usize,
    pub entities: Vec<EntityDrift>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

// ─── Pure comparators (unit-tested, no DB) ─────────────────────────────────

fn presence_token(v: &Value) -> &'static str {
    match v {
        Value::Null => "unset",
        Value::String(s) if s.is_empty() => "unset",
        _ => "set",
    }
}

/// Diff two config objects field by field. Keys present in only one side are
/// diffed against `null`. Sensitive keys are reduced to a presence token so a
/// rotated/differing secret does not read as drift and no value is surfaced.
/// Deterministic order (keys sorted).
pub fn compare_config(reference: &Value, current: &Value, sensitive: &[&str]) -> Vec<FieldDiff> {
    let empty = serde_json::Map::new();
    let r = reference.as_object().unwrap_or(&empty);
    let c = current.as_object().unwrap_or(&empty);

    let mut keys: BTreeSet<&String> = BTreeSet::new();
    keys.extend(r.keys());
    keys.extend(c.keys());

    let mut out = Vec::new();
    for k in keys {
        let rv = r.get(k).unwrap_or(&NULL);
        let cv = c.get(k).unwrap_or(&NULL);
        if sensitive.contains(&k.as_str()) {
            let (rt, ct) = (presence_token(rv), presence_token(cv));
            if rt != ct {
                out.push(FieldDiff {
                    field: k.clone(),
                    reference: Value::String(rt.into()),
                    current: Value::String(ct.into()),
                    sensitive: true,
                });
            }
        } else if rv != cv {
            out.push(FieldDiff {
                field: k.clone(),
                reference: rv.clone(),
                current: cv.clone(),
                sensitive: false,
            });
        }
    }
    out
}

pub struct SingletonResult {
    pub in_sync: bool,
    pub field_diffs: Option<Vec<FieldDiff>>,
    pub note: Option<String>,
}

/// Compare a reference server's single config row against another server's.
pub fn diff_singleton(
    reference: Option<&Value>,
    current: Option<&Value>,
    sensitive: &[&str],
) -> SingletonResult {
    match (reference, current) {
        (Some(r), Some(c)) => {
            let diffs = compare_config(r, c, sensitive);
            SingletonResult {
                in_sync: diffs.is_empty(),
                field_diffs: if diffs.is_empty() { None } else { Some(diffs) },
                note: None,
            }
        }
        (Some(_), None) => SingletonResult {
            in_sync: false,
            field_diffs: None,
            note: Some("No explicit configuration here (using defaults) while the reference has one".into()),
        },
        (None, Some(_)) => SingletonResult {
            in_sync: false,
            field_diffs: None,
            note: Some("Configured here but the reference server has none".into()),
        },
        (None, None) => SingletonResult {
            in_sync: true,
            field_diffs: None,
            note: Some("Neither server has explicit configuration (both default)".into()),
        },
    }
}

pub struct CollectionResult {
    pub in_sync: bool,
    pub only_here: Vec<String>,
    pub only_reference: Vec<String>,
    pub value_diffs: Vec<EntityValueDiff>,
}

/// Compare two keyed collections of config rows. Produces three buckets:
/// entities present only here, only on the reference, and shared-identity rows
/// whose config differs.
pub fn diff_collection(
    reference: &[(String, Value)],
    current: &[(String, Value)],
    sensitive: &[&str],
) -> CollectionResult {
    let rmap: HashMap<&str, &Value> = reference.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let cmap: HashMap<&str, &Value> = current.iter().map(|(k, v)| (k.as_str(), v)).collect();

    let mut only_here: Vec<String> = current
        .iter()
        .filter(|(k, _)| !rmap.contains_key(k.as_str()))
        .map(|(k, _)| k.clone())
        .collect();
    let mut only_reference: Vec<String> = reference
        .iter()
        .filter(|(k, _)| !cmap.contains_key(k.as_str()))
        .map(|(k, _)| k.clone())
        .collect();
    only_here.sort();
    only_here.dedup();
    only_reference.sort();
    only_reference.dedup();

    let mut shared: Vec<&str> = rmap
        .keys()
        .filter(|k| cmap.contains_key(**k))
        .copied()
        .collect();
    shared.sort();

    let mut value_diffs = Vec::new();
    for id in shared {
        let fields = compare_config(rmap[id], cmap[id], sensitive);
        if !fields.is_empty() {
            value_diffs.push(EntityValueDiff {
                identity: id.to_string(),
                fields,
            });
        }
    }

    CollectionResult {
        in_sync: only_here.is_empty() && only_reference.is_empty() && value_diffs.is_empty(),
        only_here,
        only_reference,
        value_diffs,
    }
}

/// A server's backup posture is in sync when every one of its sites has an
/// enabled backup schedule and at least one destination exists (or it has no
/// sites at all — nothing to protect).
pub fn coverage_in_sync(m: &CoverageMetrics) -> bool {
    m.total_sites == 0 || (m.unprotected == 0 && m.destinations > 0)
}

// ─── DB fetchers (config projected to JSONB in SQL) ────────────────────────

struct ServerMeta {
    name: String,
    is_local: bool,
    status: String,
    last_seen_at: Option<DateTime<Utc>>,
}

async fn fetch_server_meta(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, ServerMeta>, sqlx::Error> {
    let rows: Vec<(Uuid, String, bool, String, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id, name, is_local, status, last_seen_at \
         FROM servers WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, is_local, status, last_seen_at)| {
            (id, ServerMeta { name, is_local, status, last_seen_at })
        })
        .collect())
}

async fn fetch_alert_rules(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Value>, sqlx::Error> {
    // NULL server_id (the user's default rule) is excluded by `= ANY`.
    //
    // ⚠ CORRECTNESS (s432 review): `alert_rules` enforces only
    // `UNIQUE(user_id, server_id)`, not `UNIQUE(server_id)` — a caller other
    // than the server's owner can create a stray second row for the same
    // server_id (`routes/alerts.rs::update_server_rules` takes `server_id`
    // straight from the URL path under plain `AuthUser`, with no ownership
    // check — a real, separate gap, still open). Before this session's fix,
    // this fetcher's own `user_id = $1` filter accidentally guaranteed at
    // most one row per server_id (the caller's own); collapsing unfiltered
    // rows into a `HashMap<Uuid, Value>` keyed by server_id would silently
    // pick whichever row Postgres happens to return last if a stray row
    // exists. The join below restores "one row per server_id" by resolving
    // to the row that belongs to the server's actual registering owner,
    // rather than depending on the stray-row bug being absent.
    let rows: Vec<(Uuid, Value)> = sqlx::query_as(
        "SELECT server_id, (to_jsonb(c) - 'server_id') AS config FROM ( \
           SELECT ar.server_id, ar.cpu_threshold, ar.cpu_duration, ar.memory_threshold, ar.memory_duration, \
                  ar.disk_threshold, ar.alert_cpu, ar.alert_memory, ar.alert_disk, ar.alert_offline, \
                  ar.alert_backup_failure, ar.alert_ssl_expiry, ar.alert_service_health, ar.ssl_warning_days, \
                  ar.notify_email, ar.notify_slack_url, ar.notify_discord_url, ar.cooldown_minutes, ar.escalation_policy_id \
           FROM alert_rules ar JOIN servers s ON s.id = ar.server_id AND s.user_id = ar.user_id \
           WHERE ar.server_id = ANY($1) \
         ) c",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

async fn fetch_sites(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<(String, Value)>>, sqlx::Error> {
    let rows: Vec<(Uuid, String, Value)> = sqlx::query_as(
        "SELECT server_id, domain, (to_jsonb(c) - 'server_id' - 'domain') AS config FROM ( \
           SELECT server_id, domain, runtime, proxy_port, php_version, root_path, ssl_enabled, \
                  ssl_profile, rate_limit, max_upload_mb, php_memory_mb, php_max_workers, custom_nginx, \
                  php_preset, app_command, enabled, fastcgi_cache, redis_cache, redis_db, waf_enabled, \
                  waf_mode, csp_policy, permissions_policy, bot_protection \
           FROM sites WHERE server_id = ANY($1) \
         ) c",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(group_by_server(rows))
}

async fn fetch_crons(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<(String, Value)>>, sqlx::Error> {
    let rows: Vec<(Uuid, String, Value)> = sqlx::query_as(
        "SELECT s.server_id, (s.domain || ' · ' || cr.command) AS identity, \
                jsonb_build_object('schedule', cr.schedule, 'enabled', cr.enabled, 'label', cr.label) AS config \
         FROM crons cr JOIN sites s ON cr.site_id = s.id \
         WHERE s.server_id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(group_by_server(rows))
}

async fn fetch_backup_coverage(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, CoverageMetrics>, sqlx::Error> {
    let rows: Vec<(Uuid, i64, i64, i64)> = sqlx::query_as(
        "SELECT s.server_id, \
                COUNT(*)::bigint AS total_sites, \
                COUNT(bs.id) FILTER (WHERE bs.enabled)::bigint AS backed_up, \
                COUNT(DISTINCT bs.destination_id)::bigint AS destinations \
         FROM sites s LEFT JOIN backup_schedules bs ON bs.site_id = s.id \
         WHERE s.server_id = ANY($1) \
         GROUP BY s.server_id",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(sid, total, backed, dests)| {
            (
                sid,
                CoverageMetrics {
                    total_sites: total,
                    backed_up: backed,
                    unprotected: (total - backed).max(0),
                    destinations: dests,
                },
            )
        })
        .collect())
}

fn group_by_server(rows: Vec<(Uuid, String, Value)>) -> HashMap<Uuid, Vec<(String, Value)>> {
    let mut out: HashMap<Uuid, Vec<(String, Value)>> = HashMap::new();
    for (sid, identity, config) in rows {
        out.entry(sid).or_default().push((identity, config));
    }
    out
}

// ─── Report assembly ───────────────────────────────────────────────────────

/// Build the drift report: compare each target server against `reference_id`
/// across the four v1 entities. `targets` (already excluding the reference) are
/// the servers rendered under each entity.
///
/// ⚠ SECURITY (s432): the five fetchers below used to take a `user_id` and
/// scope every query to that caller's own registered servers — the same
/// admin-scoping bug `servers::list`/`dashboard::fleet_overview` already fix.
/// This function has exactly one caller, `routes/drift.rs::report`, which is
/// `AdminUser`-gated, so there is no narrower case to preserve: every id in
/// `all_ids` was already validated against the fleet-wide server list by that
/// caller, and the fetchers below simply resolve whatever ids they're given.
pub async fn build_report(
    pool: &PgPool,
    reference_id: Uuid,
    targets: &[Uuid],
) -> Result<DriftReport, sqlx::Error> {
    let mut all_ids: Vec<Uuid> = Vec::with_capacity(targets.len() + 1);
    all_ids.push(reference_id);
    all_ids.extend_from_slice(targets);

    let meta = fetch_server_meta(pool, &all_ids).await?;
    let ref_meta = meta.get(&reference_id);
    let reference = ServerRef {
        server_id: reference_id,
        name: ref_meta.map(|m| m.name.clone()).unwrap_or_default(),
        is_local: ref_meta.map(|m| m.is_local).unwrap_or(false),
        status: ref_meta.map(|m| m.status.clone()).unwrap_or_default(),
        last_seen_at: ref_meta.and_then(|m| m.last_seen_at),
    };

    let alert_rules = fetch_alert_rules(pool, &all_ids).await?;
    let sites = fetch_sites(pool, &all_ids).await?;
    let crons = fetch_crons(pool, &all_ids).await?;
    let coverage = fetch_backup_coverage(pool, &all_ids).await?;

    // Track which target servers drift in ≥1 entity.
    let mut drifted: HashSet<Uuid> = HashSet::new();
    let mut entities: Vec<EntityDrift> = Vec::new();

    // 1. alert_rules — SINGLETON
    {
        let ref_row = alert_rules.get(&reference_id);
        let mut servers = Vec::new();
        let mut drift_count = 0;
        for t in targets {
            let res = diff_singleton(ref_row, alert_rules.get(t), ALERT_SENSITIVE);
            if !res.in_sync {
                drift_count += 1;
                drifted.insert(*t);
            }
            servers.push(base_drift(*t, &meta, res.in_sync, res.note, |s| {
                s.field_diffs = res.field_diffs.clone();
            }));
        }
        entities.push(EntityDrift {
            entity: "alert_rules".into(),
            label: "Alert rules (monitoring posture)".into(),
            mode: "singleton".into(),
            drift_count,
            servers,
        });
    }

    // 2. sites — COLLECTION
    entities.push(build_collection_entity(
        "sites",
        "Sites (inventory + per-site config)",
        &sites,
        reference_id,
        targets,
        &meta,
        NO_SENSITIVE,
        &mut drifted,
    ));

    // 3. crons — COLLECTION
    entities.push(build_collection_entity(
        "crons",
        "Cron jobs",
        &crons,
        reference_id,
        targets,
        &meta,
        NO_SENSITIVE,
        &mut drifted,
    ));

    // 4. backup_coverage — SUMMARY
    {
        let ref_cov = coverage.get(&reference_id).copied();
        let mut servers = Vec::new();
        let mut drift_count = 0;
        for t in targets {
            let cov = coverage.get(t).copied().unwrap_or(CoverageMetrics {
                total_sites: 0,
                backed_up: 0,
                unprotected: 0,
                destinations: 0,
            });
            let in_sync = coverage_in_sync(&cov);
            if !in_sync {
                drift_count += 1;
                drifted.insert(*t);
            }
            servers.push(base_drift(*t, &meta, in_sync, None, |s| {
                s.coverage = Some(cov);
                s.reference_coverage = ref_cov;
            }));
        }
        entities.push(EntityDrift {
            entity: "backup_coverage".into(),
            label: "Backup coverage".into(),
            mode: "summary".into(),
            drift_count,
            servers,
        });
    }

    let note = if targets.is_empty() {
        Some("Only one server is available — register fleet members to detect drift.".into())
    } else {
        None
    };

    Ok(DriftReport {
        reference,
        generated_at: Utc::now(),
        servers_compared: targets.len(),
        total_drifted_servers: drifted.len(),
        entities,
        note,
    })
}

/// Construct a `ServerEntityDrift` with common fields filled, then let the
/// caller populate the mode-specific fields via `fill`.
fn base_drift(
    server_id: Uuid,
    meta: &HashMap<Uuid, ServerMeta>,
    in_sync: bool,
    note: Option<String>,
    fill: impl FnOnce(&mut ServerEntityDrift),
) -> ServerEntityDrift {
    let m = meta.get(&server_id);
    let mut s = ServerEntityDrift {
        server_id,
        name: m.map(|x| x.name.clone()).unwrap_or_default(),
        status: m.map(|x| x.status.clone()).unwrap_or_default(),
        last_seen_at: m.and_then(|x| x.last_seen_at),
        in_sync,
        note,
        field_diffs: None,
        only_here: None,
        only_reference: None,
        value_diffs: None,
        coverage: None,
        reference_coverage: None,
    };
    fill(&mut s);
    s
}

#[allow(clippy::too_many_arguments)]
fn build_collection_entity(
    entity: &str,
    label: &str,
    data: &HashMap<Uuid, Vec<(String, Value)>>,
    reference_id: Uuid,
    targets: &[Uuid],
    meta: &HashMap<Uuid, ServerMeta>,
    sensitive: &[&str],
    drifted: &mut HashSet<Uuid>,
) -> EntityDrift {
    let empty: Vec<(String, Value)> = Vec::new();
    let ref_rows = data.get(&reference_id).unwrap_or(&empty);
    let mut servers = Vec::new();
    let mut drift_count = 0;
    for t in targets {
        let cur = data.get(t).unwrap_or(&empty);
        let res = diff_collection(ref_rows, cur, sensitive);
        if !res.in_sync {
            drift_count += 1;
            drifted.insert(*t);
        }
        servers.push(base_drift(*t, meta, res.in_sync, None, |s| {
            s.only_here = Some(res.only_here);
            s.only_reference = Some(res.only_reference);
            s.value_diffs = Some(res.value_diffs);
        }));
    }
    EntityDrift {
        entity: entity.into(),
        label: label.into(),
        mode: "collection".into(),
        drift_count,
        servers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identical_configs_have_no_diff() {
        let a = json!({"cpu_threshold": 90, "alert_cpu": true});
        let b = json!({"cpu_threshold": 90, "alert_cpu": true});
        assert!(compare_config(&a, &b, NO_SENSITIVE).is_empty());
    }

    #[test]
    fn single_field_difference_is_reported() {
        let a = json!({"cpu_threshold": 90, "alert_cpu": true});
        let b = json!({"cpu_threshold": 80, "alert_cpu": true});
        let diffs = compare_config(&a, &b, NO_SENSITIVE);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].field, "cpu_threshold");
        assert_eq!(diffs[0].reference, json!(90));
        assert_eq!(diffs[0].current, json!(80));
        assert!(!diffs[0].sensitive);
    }

    #[test]
    fn sensitive_field_differing_values_are_not_drift() {
        // Two different webhook URLs are both "set" — rotating a secret must
        // not read as drift, and the value must never surface.
        let a = json!({"notify_slack_url": "https://hooks.slack.com/AAA"});
        let b = json!({"notify_slack_url": "https://hooks.slack.com/BBB"});
        assert!(compare_config(&a, &b, ALERT_SENSITIVE).is_empty());
    }

    #[test]
    fn sensitive_field_set_vs_unset_is_drift_without_value() {
        let a = json!({"notify_slack_url": "https://hooks.slack.com/AAA"});
        let b = json!({"notify_slack_url": null});
        let diffs = compare_config(&a, &b, ALERT_SENSITIVE);
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].sensitive);
        assert_eq!(diffs[0].reference, json!("set"));
        assert_eq!(diffs[0].current, json!("unset"));
    }

    #[test]
    fn key_present_on_one_side_only_diffs_against_null() {
        let a = json!({"waf_mode": "block"});
        let b = json!({});
        let diffs = compare_config(&a, &b, NO_SENSITIVE);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].field, "waf_mode");
        assert_eq!(diffs[0].current, Value::Null);
    }

    #[test]
    fn empty_string_secret_counts_as_unset() {
        let a = json!({"notify_discord_url": ""});
        let b = json!({"notify_discord_url": null});
        assert!(compare_config(&a, &b, ALERT_SENSITIVE).is_empty());
    }

    #[test]
    fn singleton_both_missing_is_in_sync() {
        let r = diff_singleton(None, None, NO_SENSITIVE);
        assert!(r.in_sync);
        assert!(r.field_diffs.is_none());
    }

    #[test]
    fn singleton_reference_only_is_drift() {
        let ref_row = json!({"cpu_threshold": 90});
        let r = diff_singleton(Some(&ref_row), None, NO_SENSITIVE);
        assert!(!r.in_sync);
        assert!(r.note.is_some());
        assert!(r.field_diffs.is_none());
    }

    #[test]
    fn singleton_equal_rows_in_sync_with_no_field_diffs() {
        let a = json!({"cpu_threshold": 90});
        let b = json!({"cpu_threshold": 90});
        let r = diff_singleton(Some(&a), Some(&b), NO_SENSITIVE);
        assert!(r.in_sync);
        assert!(r.field_diffs.is_none());
    }

    #[test]
    fn collection_identical_sets_are_in_sync() {
        let cfg = json!({"enabled": true});
        let a = vec![("a.com".to_string(), cfg.clone())];
        let b = vec![("a.com".to_string(), cfg.clone())];
        let r = diff_collection(&a, &b, NO_SENSITIVE);
        assert!(r.in_sync);
        assert!(r.only_here.is_empty() && r.only_reference.is_empty() && r.value_diffs.is_empty());
    }

    #[test]
    fn collection_presence_asymmetry_is_bucketed() {
        let cfg = json!({"enabled": true});
        let reference = vec![("shared.com".to_string(), cfg.clone()), ("refonly.com".to_string(), cfg.clone())];
        let current = vec![("shared.com".to_string(), cfg.clone()), ("hereonly.com".to_string(), cfg.clone())];
        let r = diff_collection(&reference, &current, NO_SENSITIVE);
        assert_eq!(r.only_here, vec!["hereonly.com".to_string()]);
        assert_eq!(r.only_reference, vec!["refonly.com".to_string()]);
        assert!(r.value_diffs.is_empty());
        assert!(!r.in_sync);
    }

    #[test]
    fn collection_shared_identity_value_diff() {
        let reference = vec![("a.com".to_string(), json!({"schedule": "0 3 * * *"}))];
        let current = vec![("a.com".to_string(), json!({"schedule": "0 5 * * *"}))];
        let r = diff_collection(&reference, &current, NO_SENSITIVE);
        assert!(r.only_here.is_empty() && r.only_reference.is_empty());
        assert_eq!(r.value_diffs.len(), 1);
        assert_eq!(r.value_diffs[0].identity, "a.com");
        assert_eq!(r.value_diffs[0].fields[0].field, "schedule");
        assert!(!r.in_sync);
    }

    #[test]
    fn backup_coverage_in_sync_rules() {
        // no sites → nothing to protect → in sync
        assert!(coverage_in_sync(&CoverageMetrics { total_sites: 0, backed_up: 0, unprotected: 0, destinations: 0 }));
        // all protected with a destination → in sync
        assert!(coverage_in_sync(&CoverageMetrics { total_sites: 5, backed_up: 5, unprotected: 0, destinations: 1 }));
        // an unprotected site → drift
        assert!(!coverage_in_sync(&CoverageMetrics { total_sites: 5, backed_up: 3, unprotected: 2, destinations: 1 }));
        // fully "backed up" but no destination configured → drift
        assert!(!coverage_in_sync(&CoverageMetrics { total_sites: 5, backed_up: 5, unprotected: 0, destinations: 0 }));
    }
}
