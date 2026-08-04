use sqlx::PgPool;
use uuid::Uuid;

pub async fn log_activity(
    pool: &PgPool,
    user_id: Uuid,
    user_email: &str,
    action: &str,
    target_type: Option<&str>,
    target_name: Option<&str>,
    details: Option<&str>,
    ip_address: Option<&str>,
) {
    write_row(
        pool,
        Some(user_id),
        user_email,
        action,
        target_type,
        target_name,
        details,
        ip_address,
        None,
    )
    .await
}

/// `log_activity`, naming the server the action was performed ON.
///
/// `activity_logs.server_id` has existed since the multi-server migration, with
/// an index and a foreign key, and until v2.58.0 nothing wrote it and nothing
/// read it. An unattended action that acts on one machine and records a row
/// indistinguishable from the same action on another is not an audit trail, and
/// it cannot carry a per-server cooldown: the auto-healer's service restarts
/// counted `target_name` alone, so on a fleet two hosts running a service of the
/// same name shared one budget.
///
/// Callers that act on the panel itself, or on nothing in particular, keep using
/// `log_activity` and leave the column NULL.
#[allow(clippy::too_many_arguments)]
pub async fn log_activity_on_server(
    pool: &PgPool,
    user_id: Uuid,
    user_email: &str,
    action: &str,
    target_type: Option<&str>,
    target_name: Option<&str>,
    details: Option<&str>,
    ip_address: Option<&str>,
    server_id: Option<Uuid>,
) {
    write_row(
        pool,
        Some(user_id),
        user_email,
        action,
        target_type,
        target_name,
        details,
        ip_address,
        server_id,
    )
    .await
}

/// `log_activity` for an event **no user initiated** — an unattended service
/// acting on its own schedule, or a request that failed before any account could
/// be identified.
///
/// This exists because there was already a convention for "no user", and it was
/// wrong in a way that wrote nothing at all. `activity_logs.user_id` is nullable
/// and its foreign key is `ON DELETE SET NULL`, so **NULL is the schema's own
/// sanctioned way to say "nobody"**. `Uuid::nil()` is not that: it is a non-NULL
/// value naming a `users` row that does not exist and cannot be created (every
/// `INSERT INTO users` lets `id` default to `gen_random_uuid()`), so
/// `fk_activity_logs_user` rejects the insert with 23503, the caller below
/// swallows it into a `tracing::warn!`, and the row is silently lost.
///
/// Four writers reached for that nil uuid, and none of them ever recorded
/// anything. Two were worse than silent: they read their **own** rows back as a
/// rate-limit gate, so a `COUNT(*)` that could only ever return 0 was standing in
/// for a cooldown. See the SSL renewal gate in `auto_healer::auto_renew_ssl` —
/// the fix there is to name the site's real owner, not to use this function.
///
/// Use this only where there genuinely is no user to name. `user_email` is NOT
/// NULL in the schema, so pass something an operator can read — the attempted
/// login address, or the name of the service acting.
#[allow(clippy::too_many_arguments)]
pub async fn log_activity_system(
    pool: &PgPool,
    user_email: &str,
    action: &str,
    target_type: Option<&str>,
    target_name: Option<&str>,
    details: Option<&str>,
    ip_address: Option<&str>,
    server_id: Option<Uuid>,
) {
    write_row(
        pool,
        None,
        user_email,
        action,
        target_type,
        target_name,
        details,
        ip_address,
        server_id,
    )
    .await
}

/// The single INSERT. `user_id` is `Option` here and nowhere else in the public
/// API, so a caller has to choose between naming a user and declaring there
/// isn't one — rather than inventing a third answer out of a sentinel uuid.
#[allow(clippy::too_many_arguments)]
async fn write_row(
    pool: &PgPool,
    user_id: Option<Uuid>,
    user_email: &str,
    action: &str,
    target_type: Option<&str>,
    target_name: Option<&str>,
    details: Option<&str>,
    ip_address: Option<&str>,
    server_id: Option<Uuid>,
) {
    if let Err(e) = sqlx::query(
        "INSERT INTO activity_logs (user_id, user_email, action, target_type, target_name, details, ip_address, server_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
    )
    .bind(user_id)
    .bind(user_email)
    .bind(action)
    .bind(target_type)
    .bind(target_name)
    .bind(details)
    .bind(ip_address)
    .bind(server_id)
    .execute(pool)
    .await {
        tracing::warn!("Failed to log activity '{action}': {e}");
    }
}
