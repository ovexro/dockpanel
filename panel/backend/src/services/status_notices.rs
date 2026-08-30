//! Status-page subscriber fan-out (s251).
//!
//! One worker owns every email sent to `status_page_subscribers`, for both
//! producers: monitor status transitions (`services::uptime`) and incident
//! updates (`routes::incidents`).
//!
//! Neither producer may do the fan-out itself.
//!
//! - Inline is wrong. In `uptime` the caller is a `check_monitor` task holding
//!   one of only 10 concurrency slots, and the tick loop drains the whole
//!   JoinSet before advancing — so awaiting a long fan-out there stalls ALL
//!   monitoring. In `incidents` the caller is an HTTP handler, so it stalls the
//!   operator's request (and holds a pool slot) until the fan-out finishes or
//!   the proxy times out.
//! - A detached `tokio::spawn` per event is also wrong. It loses ordering — two
//!   events one tick apart race, and subscribers can be told a monitor recovered
//!   before they are told it went down — and it loses backpressure, so a
//!   fleet-wide blip spawns hundreds of concurrent fan-outs that exhaust the
//!   20-connection pool and the mail relay at once.
//!
//! One worker draining a bounded FIFO gives both back: notices go out in the
//! order the events happened, and exactly one fan-out is ever in flight.
//!
//! s427: `status_page_subscribers` carries an `owner_id`, and every
//! [`StatusNotice`] carries the tenant its producer belongs to — the fan-out
//! below only reaches subscribers stamped with that same owner. Before this,
//! the subscriber list was global: on a multi-tenant install every verified
//! subscriber received every tenant's status notices.

use sqlx::PgPool;
use uuid::Uuid;

/// Largest subscriber fan-out one notice may send. The public subscribe
/// endpoint is unauthenticated, so this list is attacker-growable; without a
/// ceiling one notice could queue an unbounded number of serial SMTP
/// round-trips.
const MAX_SUBSCRIBER_FANOUT: i64 = 1000;

/// How many pending notices may queue before new ones are dropped. Bounded so a
/// flapping fleet cannot grow the queue without limit.
const QUEUE_DEPTH: usize = 256;

pub struct StatusNotice {
    /// What the notice is about — a monitor or incident name, for logging only.
    pub source: String,
    pub subject: String,
    pub message: String,
    /// s427: the tenant this notice's monitor/incident belongs to. The
    /// fan-out below only reaches subscribers stamped with this SAME
    /// owner_id — previously every verified subscriber got every tenant's
    /// notices regardless of who they subscribed through.
    pub owner_id: Uuid,
}

static NOTICE_TX: std::sync::OnceLock<tokio::sync::mpsc::Sender<StatusNotice>> =
    std::sync::OnceLock::new();

/// Start the worker. Idempotent — later calls are no-ops, so it is safe for a
/// supervised service to call this on every restart.
pub fn start_worker(pool: PgPool) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<StatusNotice>(QUEUE_DEPTH);
    if NOTICE_TX.set(tx).is_err() {
        return; // already started
    }

    tokio::spawn(async move {
        while let Some(notice) = rx.recv().await {
            // Bail before the fan-out if there is no mail transport at all. The
            // public subscribe endpoint auto-verifies, so verified subscribers
            // exist even on installs that never configured SMTP — without this
            // check each notice would walk the whole list and emit one failed-send
            // warning plus two settings queries per subscriber.
            let smtp_configured: Option<(String,)> =
                sqlx::query_as("SELECT value FROM settings WHERE key = 'smtp_host'")
                    .fetch_optional(&pool)
                    .await
                    .ok()
                    .flatten();
            if smtp_configured.is_none() {
                tracing::debug!(
                    "No SMTP configured — skipping subscriber notices for '{}'",
                    notice.source
                );
                continue;
            }

            let emails: Vec<(String,)> = sqlx::query_as(
                "SELECT email FROM status_page_subscribers \
                 WHERE owner_id = $1 AND verified = TRUE AND notify_incidents = TRUE \
                 ORDER BY created_at ASC LIMIT $2",
            )
            .bind(notice.owner_id)
            .bind(MAX_SUBSCRIBER_FANOUT)
            .fetch_all(&pool)
            .await
            .unwrap_or_default();

            if emails.is_empty() {
                continue;
            }

            if emails.len() as i64 == MAX_SUBSCRIBER_FANOUT {
                tracing::warn!(
                    "Status notice for '{}' capped at {MAX_SUBSCRIBER_FANOUT} subscribers — \
                     later subscribers were not emailed",
                    notice.source
                );
            }

            for (email,) in &emails {
                crate::services::notifications::send_notification(
                    &pool,
                    &crate::services::notifications::NotifyChannels {
                        email: Some(email.clone()),
                        slack_url: None,
                        discord_url: None,
                        pagerduty_key: None,
                        webhook_url: None,
                        muted_types: String::new(),
                    },
                    &notice.subject,
                    &notice.message,
                    &notice.message,
                )
                .await;
            }
        }
    });
}

/// Enqueue a notice and return immediately — never blocks the caller.
///
/// A full queue drops the notice with a warning rather than applying
/// backpressure to a monitor check or an HTTP handler: a status email is worth
/// less than timely checks and a responsive API.
pub fn enqueue(source: &str, subject: String, message: String, owner_id: Uuid) {
    let Some(tx) = NOTICE_TX.get() else {
        tracing::warn!("Status-notice worker not started; dropping '{source}' notification");
        return;
    };

    let notice = StatusNotice {
        source: source.to_string(),
        subject,
        message,
        owner_id,
    };

    if tx.try_send(notice).is_err() {
        tracing::warn!(
            "Status-notice queue full ({QUEUE_DEPTH}) — dropped notification for '{source}'"
        );
    }
}
