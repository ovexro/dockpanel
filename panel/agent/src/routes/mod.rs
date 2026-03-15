pub mod backups;
pub mod crons;
pub mod database;
pub mod deploy;
pub mod diagnostics;
pub mod php;
pub mod docker_apps;
pub mod files;
pub mod health;
pub mod iac;
pub mod logs;
pub mod mail;
pub mod nginx;
pub mod remote_backup;
pub mod security;
pub mod server_utils;
pub mod service_installer;
pub mod services;
pub mod smtp;
pub mod staging;
pub mod ssl;
pub mod system;
pub mod terminal;
pub mod updates;
pub mod wordpress;

use axum::{
    extract::Request,
    http::{Method, StatusCode},
    middleware::Next,
    response::Response,
};
use bollard::Docker;
use std::sync::Arc;
use sysinfo::System;
use tera::Tera;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState {
    pub token: String,
    pub templates: Arc<Tera>,
    pub system: Arc<Mutex<System>>,
    pub docker: Docker,
}

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

/// Auth middleware — validates Bearer token on all routes except /health.
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
            let provided = &header[7..];
            if provided == state.token {
                Ok(next.run(request).await)
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
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
