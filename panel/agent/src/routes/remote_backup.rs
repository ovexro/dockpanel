use axum::{
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;

use super::AppState;
use crate::services::remote_backup;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Deserialize)]
pub struct UploadRequest {
    pub filepath: String,
    pub destination: DestinationConfig,
}

#[derive(Deserialize)]
pub struct DestinationConfig {
    #[serde(rename = "type")]
    pub dtype: String,
    // S3/R2 fields
    pub bucket: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub path_prefix: Option<String>,
    // SFTP fields
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub key_path: Option<String>,
    pub remote_path: Option<String>,
}

#[derive(Deserialize)]
pub struct TestRequest {
    pub destination: DestinationConfig,
}

#[derive(Deserialize)]
pub struct PruneRequest {
    pub destination: DestinationConfig,
    pub domain: String,
    pub retention: usize,
}

/// POST /backups/upload — Upload a local backup file to a remote destination.
async fn upload(
    Json(body): Json<UploadRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    // Validate filepath. The prefix check alone is a LEXICAL, not a resolved,
    // guard -- "/var/backups/dockpanel/../../etc/shadow" satisfies
    // starts_with() and the OS resolves the ".." on open, same shape as
    // git_build.rs's is_valid_dockerfile/is_valid_build_context reject
    // elsewhere in this crate. ProtectSystem=strict blocks writes outside the
    // agent's ReadWritePaths allowlist, not reads, so an unguarded path here
    // would be arbitrary root-readable-file exfiltration to an
    // attacker-supplied S3/SFTP destination, not merely a write bypass.
    if !body.filepath.starts_with("/var/backups/dockpanel/") || body.filepath.contains("..") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid backup filepath"));
    }

    match body.destination.dtype.as_str() {
        "s3" => {
            let bucket = body.destination.bucket.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing bucket"))?;
            let region = body.destination.region.as_deref().unwrap_or("us-east-1");
            let endpoint = body.destination.endpoint.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing endpoint"))?;
            let access_key = body.destination.access_key.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing access_key"))?;
            let secret_key = body.destination.secret_key.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing secret_key"))?;
            let prefix = body.destination.path_prefix.as_deref().unwrap_or("");

            let url = remote_backup::upload_s3(
                &body.filepath, bucket, region, endpoint, access_key, secret_key, prefix,
            )
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

            Ok(Json(serde_json::json!({ "success": true, "url": url })))
        }
        "sftp" => {
            let host = body.destination.host.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing host"))?;
            let port = body.destination.port.unwrap_or(22);
            let username = body.destination.username.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing username"))?;
            let remote_path = body.destination.remote_path.as_deref().unwrap_or("/backups");

            let dest = remote_backup::upload_sftp(
                &body.filepath,
                host,
                port,
                username,
                body.destination.password.as_deref(),
                body.destination.key_path.as_deref(),
                remote_path,
            )
            .await
            .map_err(|e| {
                // The same rule `test_destination` states below, applied to the
                // path that matters more. A scheduled upload runs unattended, so
                // the redacted body IS the whole message an operator ever gets:
                // v2.99.0 taught the test handler to answer a precondition as a
                // 4xx and left this one answering 5xx, which means the nightly
                // failure still reads "your agent broke" on a working agent.
                let status = if e == remote_backup::SSHPASS_MISSING {
                    StatusCode::FAILED_DEPENDENCY
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                };
                err(status, &e)
            })?;

            Ok(Json(serde_json::json!({ "success": true, "destination": dest })))
        }
        other => Err(err(StatusCode::BAD_REQUEST, &format!("Unknown destination type: {other}"))),
    }
}

/// POST /backups/test-destination — Test connection to a remote destination.
async fn test_destination(
    Json(body): Json<TestRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    match body.destination.dtype.as_str() {
        "s3" => {
            let bucket = body.destination.bucket.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing bucket"))?;
            let region = body.destination.region.as_deref().unwrap_or("us-east-1");
            let endpoint = body.destination.endpoint.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing endpoint"))?;
            let access_key = body.destination.access_key.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing access_key"))?;
            let secret_key = body.destination.secret_key.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing secret_key"))?;

            // Same defaults the upload handler applies, so the test probes the
            // location the backups will actually go to rather than the bucket root.
            let prefix = body.destination.path_prefix.as_deref().unwrap_or("");

            remote_backup::test_s3(bucket, region, endpoint, access_key, secret_key, prefix)
                .await
                .map_err(|e| err(StatusCode::BAD_GATEWAY, &e))?;

            Ok(Json(serde_json::json!({ "success": true, "message": "S3 connection successful" })))
        }
        "sftp" => {
            let host = body.destination.host.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing host"))?;
            let port = body.destination.port.unwrap_or(22);
            let username = body.destination.username.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing username"))?;
            let remote_path = body.destination.remote_path.as_deref().unwrap_or("/backups");

            remote_backup::test_sftp(
                host, port, username,
                body.destination.password.as_deref(),
                body.destination.key_path.as_deref(),
                remote_path,
            )
            .await
            .map_err(|e| {
                // A missing package on this box is a precondition the caller can
                // act on, not the agent falling over. The panel keeps the body of
                // a 4xx and replaces the body of a 5xx with an incident id, so
                // answering 5xx here is what discards the remedy.
                let status = if e == remote_backup::SSHPASS_MISSING {
                    StatusCode::FAILED_DEPENDENCY
                } else {
                    StatusCode::BAD_GATEWAY
                };
                err(status, &e)
            })?;

            Ok(Json(serde_json::json!({ "success": true, "message": "SFTP connection successful" })))
        }
        other => Err(err(StatusCode::BAD_REQUEST, &format!("Unknown type: {other}"))),
    }
}

/// POST /backups/prune — Delete old remote backups, keep N newest.
async fn prune(
    Json(body): Json<PruneRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.destination.dtype != "s3" {
        // SFTP pruning requires an SSH client to list+delete remote files — not yet implemented.
        // Old SFTP backups must be pruned manually or via a cron job on the remote server.
        tracing::warn!(
            "Backup pruning skipped for {} destination (domain: {}). Only S3/R2 pruning is supported. \
             SFTP backups for '{}' must be pruned manually on the remote server.",
            body.destination.dtype, body.domain, body.domain
        );
        return Ok(Json(serde_json::json!({
            "pruned": 0,
            "message": format!(
                "Automatic pruning is not supported for {} destinations. \
                 Old backups for '{}' must be pruned manually on the remote server.",
                body.destination.dtype, body.domain
            )
        })));
    }

    let bucket = body.destination.bucket.as_deref()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing bucket"))?;
    let region = body.destination.region.as_deref().unwrap_or("us-east-1");
    let endpoint = body.destination.endpoint.as_deref()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing endpoint"))?;
    let access_key = body.destination.access_key.as_deref()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing access_key"))?;
    let secret_key = body.destination.secret_key.as_deref()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing secret_key"))?;
    let prefix = body.destination.path_prefix.as_deref().unwrap_or("");

    // List all backups for this domain
    let keys = remote_backup::list_s3(bucket, region, endpoint, access_key, secret_key, prefix)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    // Filter to this domain's backups and sort (filename contains timestamp)
    let domain_prefix = format!("{}-", body.domain);
    let mut domain_keys: Vec<&String> = keys
        .iter()
        .filter(|k| {
            k.split('/').last().map(|f| f.starts_with(&domain_prefix) && f.ends_with(".tar.gz")).unwrap_or(false)
        })
        .collect();

    // Sort by name descending (timestamps sort lexicographically)
    domain_keys.sort_by(|a, b| b.cmp(a));

    // Delete excess
    let mut pruned = 0usize;
    if domain_keys.len() > body.retention {
        for key in &domain_keys[body.retention..] {
            if let Err(e) = remote_backup::delete_s3(bucket, region, endpoint, access_key, secret_key, key).await {
                tracing::warn!("Failed to prune {key}: {e}");
            } else {
                pruned += 1;
            }
        }
    }

    Ok(Json(serde_json::json!({ "pruned": pruned, "total": domain_keys.len() })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/backups/upload", post(upload))
        .route("/backups/test-destination", post(test_destination))
        .route("/backups/prune", post(prune))
}
