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
    if let Err(e) = sqlx::query(
        "INSERT INTO activity_logs (user_id, user_email, action, target_type, target_name, details, ip_address) VALUES ($1, $2, $3, $4, $5, $6, $7)"
    )
    .bind(user_id)
    .bind(user_email)
    .bind(action)
    .bind(target_type)
    .bind(target_name)
    .bind(details)
    .bind(ip_address)
    .execute(pool)
    .await {
        tracing::warn!("Failed to log activity '{action}': {e}");
    }
}
