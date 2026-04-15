pub mod backup_verify;
pub mod backups;
pub mod cms;
pub mod database_backup;
pub mod crons;
pub mod database;
pub mod deploy;
pub mod diagnostics;
pub mod telemetry;
pub mod git_build;
pub mod php;
pub mod docker_apps;
pub mod files;
pub mod health;
pub mod iac;
pub mod image_scan;
pub mod logs;
pub mod sbom;
pub mod mail;
pub mod migration;
pub mod nginx;
pub mod remote_backup;
pub mod security;
pub mod server_utils;
pub mod service_installer;
pub mod services;
pub mod smtp;
pub mod staging;
pub mod ssl;
pub mod traefik;
pub mod system;
pub mod terminal;
pub mod updates;
pub mod volume_backup;
pub mod wordpress;

use axum::{
    extract::Request,
    http::{Method, StatusCode},
    middleware::Next,
    response::Response,
};
use bollard::Docker;
use std::collections::HashMap;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use sysinfo::System;
use tera::Tera;
use tokio::sync::{Mutex, RwLock};

/// Snapshot of network counters for rate calculation.
pub struct NetworkSnapshot {
    /// Per-interface (rx_bytes, tx_bytes) at the time of the snapshot.
    pub readings: HashMap<String, (u64, u64)>,
    /// When the snapshot was taken.
    pub timestamp: std::time::Instant,
}

#[derive(Clone)]
pub struct AppState {
    pub token: Arc<RwLock<String>>,
    /// Previous token kept valid for a grace period during rotation.
    pub previous_token: Arc<RwLock<Option<(String, std::time::Instant)>>>,
    pub templates: Arc<Tera>,
    pub system: Arc<Mutex<System>>,
    pub docker: Docker,
    pub network_snapshot: Arc<Mutex<Option<NetworkSnapshot>>>,
}

/// Grace period (in seconds) during which the old token remains valid after rotation.
const TOKEN_ROTATION_GRACE_SECS: u64 = 60;

/// Validate a domain name format (shared across route modules).
pub fn is_valid_domain(domain: &str) -> bool {
    if domain.is_empty() || domain.len() > 253 {
        return false;
    }
    domain.split('.').all(|part| {
        !part.is_empty()
            && part.len() <= 63
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !part.starts_with('-')
            && !part.ends_with('-')
    }) && domain.contains('.')
}

/// Validate a resource name (database, app, container, etc.).
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validate a Docker container ID (hex string, 1–64 chars).
pub fn is_valid_container_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_domains() {
        assert!(is_valid_domain("example.com"));
        assert!(is_valid_domain("sub.example.com"));
        assert!(is_valid_domain("my-site.example.com"));
    }

    #[test]
    fn invalid_domains() {
        assert!(!is_valid_domain(""));
        assert!(!is_valid_domain("localhost"));
        assert!(!is_valid_domain("../etc/passwd"));
        assert!(!is_valid_domain("-bad.com"));
    }

    #[test]
    fn valid_names() {
        assert!(is_valid_name("mydb"));
        assert!(is_valid_name("my-app-123"));
    }

    #[test]
    fn invalid_names() {
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("-bad"));
        assert!(!is_valid_name("has space"));
    }

    #[test]
    fn valid_container_ids() {
        assert!(is_valid_container_id("abc123def456"));
        assert!(is_valid_container_id(&"a".repeat(64)));
    }

    #[test]
    fn invalid_container_ids() {
        assert!(!is_valid_container_id(""));
        assert!(!is_valid_container_id("not-hex!"));
        assert!(!is_valid_container_id(&"a".repeat(65)));
    }
}

/// Auth middleware — validates Bearer token on all routes except /health.
/// Uses constant-time comparison to prevent timing attacks.
/// Supports a grace period for the previous token during rotation.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Skip auth for health endpoint
    if request.uri().path() == "/health" {
        return Ok(next.run(request).await);
    }

    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let provided = header[7..].as_bytes();

            // Check current token (constant-time)
            let current = state.token.read().await;
            if provided.ct_eq(current.as_bytes()).into() {
                drop(current);
                return Ok(next.run(request).await);
            }
            drop(current);

            // Check previous token within grace period
            let prev = state.previous_token.read().await;
            if let Some((ref old_token, ref rotated_at)) = *prev {
                if rotated_at.elapsed().as_secs() < TOKEN_ROTATION_GRACE_SECS
                    && provided.ct_eq(old_token.as_bytes()).into()
                {
                    drop(prev);
                    return Ok(next.run(request).await);
                }
            }

            Err(StatusCode::UNAUTHORIZED)
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Rotate the agent token. Returns the new token.
/// The old token remains valid for TOKEN_ROTATION_GRACE_SECS seconds.
pub async fn rotate_token(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    let new_token = uuid::Uuid::new_v4().to_string();

    // Move current token to previous (with grace period)
    let old_token = state.token.read().await.clone();
    {
        let mut prev = state.previous_token.write().await;
        *prev = Some((old_token, std::time::Instant::now()));
    }

    // Write new token to disk atomically
    let token_path = format!("{}/agent.token", super::CONFIG_DIR);
    let tmp_path = format!("{}.tmp", token_path);
    if let Err(e) = std::fs::write(&tmp_path, &new_token) {
        tracing::error!("Failed to write temp token file: {e}");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    if let Err(e) = std::fs::rename(&tmp_path, &token_path) {
        tracing::error!("Failed to rename token file: {e}");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).ok();
    }

    // Update in-memory token
    {
        let mut current = state.token.write().await;
        *current = new_token.clone();
    }

    tracing::info!("Agent token rotated successfully");
    Ok(axum::Json(serde_json::json!({ "new_token": new_token })))
}

/// Audit logging middleware — logs all state-modifying requests (POST, PUT, DELETE).
pub async fn audit_middleware(
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();

    // Only audit state-modifying methods
    if method != Method::POST && method != Method::PUT && method != Method::DELETE {
        return next.run(request).await;
    }

    let path = request.uri().path().to_string();
    let source_ip = request
        .headers()
        .get("x-forwarded-for")
        .or_else(|| request.headers().get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    let response = next.run(request).await;
    let status = response.status().as_u16();

    if status < 400 {
        tracing::info!(
            target: "audit",
            method = %method,
            path = %path,
            source_ip = %source_ip,
            status = status,
            "Request completed"
        );
    } else {
        tracing::warn!(
            target: "audit",
            method = %method,
            path = %path,
            source_ip = %source_ip,
            status = status,
            "Request failed"
        );
    }

    response
}
