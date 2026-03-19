use sqlx::PgPool;
use tokio::sync::broadcast;
use std::time::Duration;
use crate::services::agent::AgentClient;

/// Background task: cleanup expired preview environments every 5 minutes.
pub async fn run(db: PgPool, agent: AgentClient, mut shutdown: broadcast::Receiver<()>) {
    tracing::info!("Preview cleanup service started");

    let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 minutes

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown.recv() => {
                tracing::info!("Preview cleanup service shutting down");
                break;
            }
        }

        if let Err(e) = cleanup_expired_previews(&db, &agent).await {
            tracing::warn!("Preview cleanup error: {e}");
        }
    }
}

async fn cleanup_expired_previews(db: &PgPool, agent: &AgentClient) -> Result<(), String> {
    // Find expired previews: where updated_at + ttl_hours has passed
    // Join with git_deploys to get preview_ttl_hours
    let expired: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
        "SELECT p.id, p.container_name, p.branch \
         FROM git_previews p \
         JOIN git_deploys d ON d.id = p.git_deploy_id \
         WHERE p.status = 'running' \
         AND d.preview_ttl_hours > 0 \
         AND p.updated_at < NOW() - MAKE_INTERVAL(hours => d.preview_ttl_hours)"
    )
    .fetch_all(db)
    .await
    .map_err(|e| e.to_string())?;

    for (id, container_name, branch) in &expired {
        tracing::info!("Cleaning up expired preview: {container_name} (branch: {branch})");

        // Try to stop and remove the container
        if let Err(e) = agent.post("/git/cleanup", Some(serde_json::json!({
            "name": container_name,
        }))).await {
            tracing::warn!("Failed to cleanup preview container {container_name}: {e}");
        }

        // Delete the preview record
        if let Err(e) = sqlx::query("DELETE FROM git_previews WHERE id = $1")
            .bind(id)
            .execute(db)
            .await
        {
            tracing::warn!("Failed to delete preview record {id}: {e}");
        }
    }

    if !expired.is_empty() {
        tracing::info!("Cleaned up {} expired previews", expired.len());
    }

    // Also clean up stuck previews (deploying/failed for > 1 hour)
    let stuck: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, container_name FROM git_previews \
         WHERE status IN ('deploying', 'failed') \
         AND updated_at < NOW() - INTERVAL '1 hour'"
    )
    .fetch_all(db)
    .await
    .map_err(|e| e.to_string())?;

    for (id, container_name) in &stuck {
        if let Err(e) = agent.post("/git/cleanup", Some(serde_json::json!({
            "name": container_name,
        }))).await {
            tracing::warn!("Failed to cleanup stuck preview container {container_name}: {e}");
        }
        if let Err(e) = sqlx::query("DELETE FROM git_previews WHERE id = $1")
            .bind(id)
            .execute(db)
            .await
        {
            tracing::warn!("Failed to delete stuck preview record {id}: {e}");
        }
    }

    Ok(())
}
