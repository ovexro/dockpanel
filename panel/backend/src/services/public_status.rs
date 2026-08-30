//! The one gate the unauthenticated status-page surface passes through.
//!
//! Four routes answer with no authentication at all — `/api/status-page`,
//! `/api/status-page/public`, `/api/status-page/subscribe` and
//! `/api/status-page/unsubscribe` — and until v2.70.0 exactly **one** of them
//! checked anything:
//!
//! - `monitors::status_page` read `settings.status_page_enabled` and failed
//!   closed. This is the switch the operator UI writes and the guide documents.
//! - `incidents::public_status_page` read a *different* flag,
//!   `status_page_config.enabled`, which defaults TRUE in the column **and**
//!   again in the `unwrap_or` used when the table has no row at all. No
//!   migration seeds that table, so it failed open on a fresh install twice
//!   over.
//! - `subscribe` and `unsubscribe` read nothing, so an unauthenticated caller
//!   could write rows into `status_page_subscribers` (as `verified = TRUE`) and
//!   delete anyone else's, on an install whose status page was never turned on.
//!
//! The consequence was not theoretical. `/status` is served by
//! `public_status_page`, not by the documented endpoint, so every install
//! published its alert engine's output — `service_down` and `disk` incidents
//! naming the operator's own hosts and services — from first boot, with no
//! operator action, while `docs/guides/status-page.md` promised that a disabled
//! status page returns 404 and the only control for the flag that actually
//! governed it was an unlabelled checkbox on a different screen.
//!
//! So the publish decision lives here, in one function, and every
//! unauthenticated status-page route calls it. Adding a fifth public route
//! without calling it is the regression this module exists to make visible:
//! `tests/status-page-gate-pin-e2e.sh` fails the build when a route registered
//! on the public `/api/status-page*` surface does not reach this gate.
//!
//! `status_page_config.enabled` survives as a *secondary* switch — it hides the
//! enhanced page while leaving the monitor endpoint up — but it is no longer a
//! publish decision, which is why its DEFAULT TRUE is now harmless and
//! deliberately left alone.

use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{err, ApiError};

/// Is anything published on the unauthenticated status-page surface?
///
/// Absent, unreadable, or any value other than the literal `"true"` is **off**.
/// A decision to publish operational state to the internet is never made by a
/// default, and never by a query that failed.
pub async fn is_enabled(pool: &PgPool) -> bool {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'status_page_enabled'",
    )
    .fetch_optional(pool)
    .await
    .unwrap_or(None);

    means_enabled(row.as_ref().map(|(v,)| v.as_str()))
}

/// The decision itself, extracted so the fail-closed property is testable
/// without a database — the same reason `alert_engine::ssl_decision` exists.
///
/// `None` covers both "no such setting" and "the query failed", because the
/// caller collapses an error into `None`: a panel that cannot read its own
/// settings must not conclude that it should publish.
fn means_enabled(value: Option<&str>) -> bool {
    value.map(|v| v == "true").unwrap_or(false)
}

/// `is_enabled`, as the guard clause a public handler starts with.
///
/// Answers 404 rather than 403 so a disabled status page is indistinguishable
/// from an install that never had one — the same answer the documented
/// endpoint has always given, and the one the guide promises.
pub async fn require_enabled(pool: &PgPool) -> Result<(), ApiError> {
    if is_enabled(pool).await {
        Ok(())
    } else {
        Err(err(StatusCode::NOT_FOUND, "Status page not enabled"))
    }
}

/// The tenant `routes::incidents::public_status_page` currently publishes —
/// same tie-break, extracted so `subscribe`/`unsubscribe`/`list_subscribers`
/// agree with the public page on who "the" status page belongs to instead of
/// treating `status_page_subscribers` as install-wide.
///
/// Falls back to the install's very first user when no `status_page_config`
/// row exists at all — reachable because `status_page_enabled` (checked by
/// `is_enabled` above) lives on a different settings screen than
/// `status_page_config`, so a visitor can reach `/subscribe` before an admin
/// has ever touched status-page settings. `None` only if `users` itself is
/// empty, which cannot happen on a running install.
pub async fn resolve_current_status_page_owner(pool: &PgPool) -> Option<Uuid> {
    let from_config: Option<(Uuid,)> = sqlx::query_as(
        "SELECT user_id FROM status_page_config ORDER BY created_at ASC, id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if let Some((uid,)) = from_config {
        return Some(uid);
    }

    let earliest_user: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users ORDER BY created_at ASC LIMIT 1")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    earliest_user.map(|(uid,)| uid)
}

#[cfg(test)]
mod tests {
    use super::means_enabled;

    #[test]
    fn the_literal_true_is_the_only_thing_that_publishes() {
        assert!(means_enabled(Some("true")));
    }

    #[test]
    fn an_absent_setting_is_off() {
        // The shape a fresh install is in, and the one that used to publish:
        // no row anywhere, and the enhanced page defaulted to on regardless.
        assert!(!means_enabled(None));
    }

    #[test]
    fn a_query_that_failed_is_off() {
        // `is_enabled` collapses a DB error into None. A panel that cannot read
        // its own settings must not conclude that it should publish.
        assert!(!means_enabled(None));
    }

    #[test]
    fn nothing_truthy_but_unequal_publishes() {
        // A settings value is operator-editable and arrives from several
        // writers; anything other than the exact literal is a refusal, not an
        // invitation to guess.
        for v in ["", "false", "TRUE", "True", "1", "yes", "on", " true", "true "] {
            assert!(!means_enabled(Some(v)), "{v:?} must not enable the status page");
        }
    }
}
