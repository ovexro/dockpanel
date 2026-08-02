use sqlx::PgPool;
use tokio::sync::broadcast;
use std::time::Duration;
use crate::services::agent::AgentRegistry;

/// Background task: cleanup expired preview environments every 5 minutes.
pub async fn run(db: PgPool, agents: AgentRegistry, mut shutdown: broadcast::Receiver<()>) {
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

        if let Err(e) = cleanup_expired_previews(&db, &agents).await {
            tracing::warn!("Preview cleanup error: {e}");
        }
    }
}

async fn cleanup_expired_previews(db: &PgPool, agents: &AgentRegistry) -> Result<(), String> {
    // Find expired previews: where updated_at + ttl_hours has passed
    // Join with git_deploys to get preview_ttl_hours
    //
    // `d.server_id` comes along for the ride. `git_previews` carries no server
    // of its own, but a preview belongs to exactly one git deploy and that row's
    // `server_id` is NOT NULL — so the JOIN this query already performs is
    // enough to name the host, and no migration is needed. Without it the sweep
    // read every server's previews and tore down containers on whichever box the
    // panel runs on: a container name that exists on both hosts is destroyed on
    // the wrong one, and the row is then deleted as if the teardown had worked.
    let expired: Vec<(uuid::Uuid, String, String, Option<String>, i32, uuid::Uuid)> = sqlx::query_as(
        "SELECT p.id, p.container_name, p.branch, p.domain, p.host_port, d.server_id \
         FROM git_previews p \
         JOIN git_deploys d ON d.id = p.git_deploy_id \
         WHERE p.status = 'running' \
         AND d.preview_ttl_hours > 0 \
         AND p.updated_at < NOW() - MAKE_INTERVAL(hours => d.preview_ttl_hours)"
    )
    .fetch_all(db)
    .await
    .map_err(|e| e.to_string())?;

    for (id, container_name, branch, domain, host_port, server_id) in &expired {
        let agent = match agents.for_server(*server_id).await {
            Ok(a) => a,
            Err(e) => {
                // Keep the row, exactly as an agent-side failure does below: an
                // unreachable server is a reason to retry, never a reason to
                // tear the container down somewhere else.
                tracing::warn!(
                    "Preview {container_name} belongs to server {server_id}, which is \
                     unreachable ({e}) — not cleaning up. Refusing to act on a different host."
                );
                continue;
            }
        };

        tracing::info!("Cleaning up expired preview: {container_name} (branch: {branch}) on server {server_id}");

        // One door for every preview teardown: it resolves the stored name into
        // the space it was created in, and carries the row's own domain and port
        // so the vhost can still be released when the container is already gone.
        let body = crate::routes::git_deploys::preview_cleanup_body(
            container_name,
            domain.as_deref(),
            Some(*host_port),
        );
        if let Err(e) = agent.post("/git/cleanup", Some(body)).await {
            // KEEP THE ROW. It is the only record that this container, this
            // port and this vhost exist — deleting it anyway frees the port for
            // reallocation while the container still holds it, and leaves the
            // container invisible to every list, cleanup and delete path there
            // is. The sweep runs every five minutes and the agent reports
            // success when the container is already gone, so retrying is cheap
            // and terminates.
            tracing::warn!(
                "Failed to cleanup preview container {container_name}: {e}. Keeping the \
                 git_previews row so the next sweep retries rather than orphaning it."
            );
            continue;
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

    // Also clean up stuck previews (deploying/failed for > 1 hour).
    //
    // `preview_ttl_hours = 0` is the documented opt-out from automatic preview
    // cleanup, and the TTL loop above honours it. This one did not join
    // git_deploys at all, so the operator who switched auto-cleanup off still
    // had previews destroyed — just by the other sweep, an hour after a failed
    // deploy, which is exactly when the container is worth keeping to look at.
    let stuck: Vec<(uuid::Uuid, String, Option<String>, i32, uuid::Uuid)> = sqlx::query_as(
        "SELECT p.id, p.container_name, p.domain, p.host_port, d.server_id FROM git_previews p \
         JOIN git_deploys d ON d.id = p.git_deploy_id \
         WHERE p.status IN ('deploying', 'failed') \
         AND d.preview_ttl_hours > 0 \
         AND p.updated_at < NOW() - INTERVAL '1 hour'"
    )
    .fetch_all(db)
    .await
    .map_err(|e| e.to_string())?;

    for (id, container_name, domain, host_port, server_id) in &stuck {
        let agent = match agents.for_server(*server_id).await {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(
                    "Stuck preview {container_name} belongs to server {server_id}, which is \
                     unreachable ({e}) — not cleaning up. Refusing to act on a different host."
                );
                continue;
            }
        };
        let body = crate::routes::git_deploys::preview_cleanup_body(
            container_name,
            domain.as_deref(),
            Some(*host_port),
        );
        if let Err(e) = agent.post("/git/cleanup", Some(body)).await {
            tracing::warn!(
                "Failed to cleanup stuck preview container {container_name}: {e}. Keeping the \
                 git_previews row so the next sweep retries rather than orphaning it."
            );
            continue;
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
