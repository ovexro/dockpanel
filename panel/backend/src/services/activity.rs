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
    log_activity_on_server(
        pool,
        user_id,
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
