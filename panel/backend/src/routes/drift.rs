//! Phase 4 W5: fleet configuration-drift admin API (read-only).
//!
//! Two admin endpoints:
//!   - `GET /api/drift/servers`               — the comparable server list (picker source).
//!   - `GET /api/drift?reference=&targets=`   — the computed `DriftReport`.
//!
//! Pure reads from the hub DB — no writes, no scheduler, no remote agent call.
//! Reconcile/push is deferred to W5.2 (see `services::drift`).

use std::collections::HashSet;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{err, internal_error, ApiError};
use crate::services::drift::{self, DriftReport};
use crate::AppState;

#[derive(Serialize)]
pub struct DriftServer {
    pub id: Uuid,
    pub name: String,
    pub is_local: bool,
    pub status: String,
    pub last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    pub agent_version: Option<String>,
}

/// GET /api/drift/servers — comparable servers, local first. Drives the
/// reference/target pickers in the UI.
///
/// ⚠ SECURITY (s432): was `WHERE user_id = $1`, the same admin-scoping bug
/// class `servers::list` and `dashboard::fleet_overview` already document and
/// fix — `servers.user_id` names whoever registered the row, not a tenant
/// boundary. This route only ever takes `AdminUser`, so (as with
/// `fleet_overview`) there is no narrower case to preserve: the comparable
/// servers are simply every server on the install.
pub async fn servers(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<DriftServer>>, ApiError> {
    let rows: Vec<(Uuid, String, bool, String, Option<chrono::DateTime<chrono::Utc>>, Option<String>)> =
        sqlx::query_as(
            "SELECT id, name, is_local, status, last_seen_at, agent_version \
             FROM servers ORDER BY is_local DESC, created_at DESC LIMIT 500",
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("drift servers", e))?;

    let out = rows
        .into_iter()
        .map(|(id, name, is_local, status, last_seen_at, agent_version)| DriftServer {
            id,
            name,
            is_local,
            status,
            last_seen_at,
            agent_version,
        })
        .collect();
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct DriftQuery {
    /// Reference server. Defaults to the local server (or newest) when absent.
    pub reference: Option<Uuid>,
    /// Comma-separated target server ids. Absent → every other server.
    pub targets: Option<String>,
}

/// GET /api/drift — compute the fleet configuration-drift report.
///
/// ⚠ SECURITY (s432): the candidate-server query was `WHERE user_id = $1`, the
/// same admin-scoping bug as `servers()` above — see that function's comment.
/// `AdminUser`-only, so the comparable set is fleet-wide.
pub async fn report(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Query(q): Query<DriftQuery>,
) -> Result<Json<DriftReport>, ApiError> {
    // The comparable servers, local first — used to validate ids and to
    // default the reference/target selection.
    let all: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM servers ORDER BY is_local DESC, created_at DESC LIMIT 500",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("drift server ids", e))?;

    if all.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "No servers available to compare"));
    }
    let valid: HashSet<Uuid> = all.iter().map(|(id,)| *id).collect();

    let reference = match q.reference {
        Some(r) => {
            if !valid.contains(&r) {
                return Err(err(StatusCode::NOT_FOUND, "Reference server not found"));
            }
            r
        }
        None => all[0].0, // local server (or newest) — ordered is_local DESC first
    };

    let targets: Vec<Uuid> = match q.targets.as_deref() {
        Some(s) if !s.trim().is_empty() => {
            let mut seen = HashSet::new();
            let mut out = Vec::new();
            for part in s.split(',') {
                let p = part.trim();
                if p.is_empty() {
                    continue;
                }
                let id = Uuid::parse_str(p)
                    .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid target server id"))?;
                if id != reference && valid.contains(&id) && seen.insert(id) {
                    out.push(id);
                }
            }
            out
        }
        _ => all
            .iter()
            .map(|(id,)| *id)
            .filter(|id| *id != reference)
            .collect(),
    };

    let report = drift::build_report(&state.db, reference, &targets)
        .await
        .map_err(|e| internal_error("drift report", e))?;

    Ok(Json(report))
}
