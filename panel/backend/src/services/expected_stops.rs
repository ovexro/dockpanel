//! Deliberate container stops — the one place that records, reads and clears them.
//!
//! The alert engine cannot otherwise tell "the operator stopped this" from "this
//! crashed": it sees `exited`/`dead` in a 120-second poll of the agent's `/apps`
//! and fires `container_down`. Every writer that stops a managed container on
//! purpose records a row here; the engine reads it before firing and deletes it
//! the moment the container is observed in any state that is not exited/dead.
//!
//! Keyed on `(server_id, container_name)` — the same key `alert_state` uses for
//! this alert type — never on `container_id`, which changes on every app Update
//! (`update_app`/`change_container_image`/`update_env` re-create the container)
//! and is not unique across a cloned fleet member. See the migration for the
//! full reasoning.

use sqlx::PgPool;
use std::collections::HashSet;
use uuid::Uuid;

/// Why a container is stopped. Stored verbatim so the UI can say it in words.
pub const REASON_OPERATOR_STOP: &str = "operator_stop";
pub const REASON_MANUAL_SLEEP: &str = "manual_sleep";
pub const REASON_AUTO_SLEEP: &str = "auto_sleep";
pub const REASON_STACK_STOP: &str = "stack_stop";
/// Update/change-image/edit-env recreate the container — `docker rm` then
/// `docker create` (or its blue-green equivalent) — which is genuinely
/// exited/absent for part of that window even though nobody asked it to stay
/// down. Recorded BEFORE the agent call starts, unlike every other reason
/// here: those record only after a stop has already succeeded, but a
/// recreate's whole point is to interrupt a running container on purpose, so
/// the healer must be blind to the gap from the moment it opens, not after.
/// [[project_dockpanel_tech_debt_p185]] carry G.
pub const REASON_RECREATE: &str = "recreate_in_progress";

/// Record that this container was stopped on purpose.
///
/// ⚠ Call this only AFTER the stop has actually succeeded. Recording it first
/// and having the agent refuse would mark a still-RUNNING container as
/// expectedly stopped, and the next genuine crash would be suppressed.
pub async fn record(
    pool: &PgPool,
    server_id: Uuid,
    container_name: &str,
    reason: &str,
    actor_email: Option<&str>,
) {
    let res = sqlx::query(
        "INSERT INTO container_expected_stops (server_id, container_name, reason, actor_email, stopped_at) \
         VALUES ($1, $2, $3, $4, NOW()) \
         ON CONFLICT (server_id, container_name) DO UPDATE SET \
             reason = $3, actor_email = $4, stopped_at = NOW()",
    )
    .bind(server_id)
    .bind(container_name)
    .bind(reason)
    .bind(actor_email)
    .execute(pool)
    .await;

    if let Err(e) = res {
        // Deliberately not fatal: failing to record an expectation costs a
        // spurious alert, while failing the operator's stop costs them the
        // action they asked for. But it must be visible — a silent miss here
        // looks exactly like the defect this table exists to fix.
        tracing::warn!(
            "Could not record expected stop for '{container_name}' on server {server_id}: {e}"
        );
    }
}

/// Forget the expectation — the container is running again, or is being started.
pub async fn clear(pool: &PgPool, server_id: Uuid, container_name: &str) {
    let _ = sqlx::query(
        "DELETE FROM container_expected_stops WHERE server_id = $1 AND container_name = $2",
    )
    .bind(server_id)
    .bind(container_name)
    .execute(pool)
    .await;
}

/// Clear from OBSERVATION, without racing a stop recorded mid-sweep.
///
/// `check_container_health` takes ONE `/apps` snapshot and then walks it with
/// awaited database and notification calls per container, so several seconds can
/// pass between the observation and this call. In that window the operator may
/// have pressed Stop. Deleting unconditionally would discard an expectation that
/// is NEWER than the evidence being used to discard it, and the next sweep would
/// then fire the alert this table exists to prevent.
///
/// `observed_at` is stamped before the snapshot is fetched, so only an
/// expectation that predates the observation is cleared.
pub async fn clear_if_older_than(
    pool: &PgPool,
    server_id: Uuid,
    container_name: &str,
    observed_at: chrono::DateTime<chrono::Utc>,
) {
    let _ = sqlx::query(
        "DELETE FROM container_expected_stops \
         WHERE server_id = $1 AND container_name = $2 AND stopped_at < $3",
    )
    .bind(server_id)
    .bind(container_name)
    .bind(observed_at)
    .execute(pool)
    .await;
}

/// Forget every expectation for a container this host no longer reports.
///
/// [`clear_if_older_than`] above runs from OBSERVING a container alive, so it
/// can only ever reach a container that still exists. A REMOVED one is never
/// observed again in any state, and nothing else reaches its row: the engine
/// keeps skipping its `container_down` branch, the auto-heal restart leg keeps
/// skipping it, and the Apps page keeps calling it "stopped on purpose" and
/// naming whoever stopped the container that is gone. Not for a container that
/// is absent — for the NEXT container of that name, which is the one that
/// crashed.
///
/// Removing an app and redeploying it under the same name is the supported way
/// to rebuild it while keeping its data (the data tree is keyed on the app's own
/// name, not on the container), so the container this silences is not a corner
/// case. It is the repair loop: stop it because it is misbehaving, remove it,
/// redeploy it, and it is still misbehaving.
///
/// `alert_engine` runs the identical sweep for `alert_state` at the same call
/// site, for the identical reason and after an identical live incident that ran
/// four months. This is that sweep's missing twin.
///
/// ⚠ `observed` MUST be the host's complete listing, and an empty one does
/// nothing. `container_name <> ALL('{}')` is true for every row, so a listing
/// that is empty because the host could not be read would clear every
/// expectation at once — and the next sweep would fire `container_down` for
/// every container the operator had deliberately stopped, which is the defect
/// this table was created to remove. An empty listing is the one observation
/// that cannot tell "nothing is here" from "I can see nothing", and the cost of
/// reading it wrong is not symmetric. A host with no containers has nothing for
/// an expectation to suppress anyway; the removal doors clear their own row.
///
/// `stopped_at < observed_at` is the guard [`clear_if_older_than`] carries, for
/// the same race: the snapshot is taken once and walked with awaited calls per
/// container, so an expectation recorded inside that window is NEWER than the
/// evidence being used to discard it.
pub async fn clear_absent(
    pool: &PgPool,
    server_id: Uuid,
    observed: &[String],
    observed_at: chrono::DateTime<chrono::Utc>,
) {
    if observed.is_empty() {
        return;
    }

    let _ = sqlx::query(
        "DELETE FROM container_expected_stops \
         WHERE server_id = $1 AND stopped_at < $2 AND container_name <> ALL($3)",
    )
    .bind(server_id)
    .bind(observed_at)
    .bind(observed)
    .execute(pool)
    .await;
}

/// Every container this host is expected to be holding stopped.
///
/// Loaded once per member per sweep rather than queried per container: the
/// health check already walks every container the host reports, and a query
/// each would multiply the sweep's database round-trips by the app count.
pub async fn expected_on_server(pool: &PgPool, server_id: Uuid) -> HashSet<String> {
    sqlx::query_as::<_, (String,)>(
        "SELECT container_name FROM container_expected_stops WHERE server_id = $1",
    )
    .bind(server_id)
    .fetch_all(pool)
    .await
    .map(|rows| rows.into_iter().map(|(n,)| n).collect())
    .unwrap_or_default()
}

/// The reason a container is stopped, for the operator-facing surfaces.
pub async fn reasons_on_server(pool: &PgPool, server_id: Uuid) -> Vec<(String, String)> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT container_name, reason FROM container_expected_stops WHERE server_id = $1",
    )
    .bind(server_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// Close out an alert the container had already raised before it was stopped.
///
/// The sequence this exists for is the common one, and it is the reverse of the
/// one the suppression models: the container CRASHES, `container_down` fires and
/// an incident opens, and THEN the operator presses Stop to take it out of
/// service while they investigate. From that moment the engine's exited arm is
/// skipped, so nothing re-evaluates the row — the recovery arm cannot resolve it
/// (the container is deliberately down) and the stale sweep cannot either (an
/// exited container is still listed by the agent). The row would stay 'firing'
/// for ever, escalating for seven days, with the incident published throughout.
///
/// So recording an expectation is also a resolve point.
pub async fn resolve_open_container_down(pool: &PgPool, server_id: Uuid, container_name: &str) {
    let owner: Option<(Uuid,)> = sqlx::query_as("SELECT user_id FROM servers WHERE id = $1")
        .bind(server_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let Some((user_id,)) = owner else { return };

    let firing: Option<(String,)> = sqlx::query_as(
        "SELECT current_state FROM alert_state \
         WHERE server_id = $1 AND alert_type = 'container_down' AND state_key = $2",
    )
    .bind(server_id)
    .bind(container_name)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    if firing.as_ref().map(|s| s.0.as_str()) != Some("firing") {
        return;
    }

    let _ = sqlx::query(
        "UPDATE alert_state SET current_state = 'ok', fired_at = NULL, last_notified_at = NULL \
         WHERE server_id = $1 AND alert_type = 'container_down' AND state_key = $2",
    )
    .bind(server_id)
    .bind(container_name)
    .execute(pool)
    .await;

    crate::services::notifications::resolve_alert(
        pool,
        user_id,
        Some(server_id),
        None,
        "container_down",
        container_name,
        &format!("Container '{container_name}' was stopped deliberately"),
        &format!(
            "Docker container '{container_name}' is stopped because it was asked to be. \
             The earlier alert has been resolved."
        ),
    )
    .await;

    // `resolve_alert` writes to `alerts` and nothing else — it has no reference
    // to `managed_incidents` — so an incident auto-opened for this container
    // would otherwise stay `investigating` on the public status page until a
    // human closed it by hand. Matched on the EXACT generated title for both
    // terminal states, never a LIKE, so a container whose name is a prefix of
    // another's cannot close its neighbour's incident.
    for state in ["exited", "dead"] {
        let _ = sqlx::query(
            "UPDATE managed_incidents SET status = 'resolved', resolved_at = NOW() \
             WHERE user_id = $1 AND title = $2 AND status NOT IN ('resolved', 'postmortem')",
        )
        .bind(user_id)
        .bind(format!("Container '{container_name}' is {state}"))
        .execute(pool)
        .await;
    }
}
