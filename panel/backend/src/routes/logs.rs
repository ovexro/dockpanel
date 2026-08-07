use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use jsonwebtoken::{encode, EncodingKey, Header};

use crate::auth::{AuthUser, AdminUser, ServerScope};
use crate::error::{internal_error, err, agent_error, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct LogQuery {
    #[serde(rename = "type")]
    pub log_type: Option<String>,
    pub lines: Option<u32>,
    pub filter: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    #[serde(rename = "type")]
    pub log_type: Option<String>,
    pub pattern: Option<String>,
    pub max: Option<u32>,
}

#[derive(serde::Deserialize)]
pub struct StreamTokenQuery {
    pub site_id: Option<String>,
    #[serde(rename = "type")]
    pub log_type: Option<String>,
}

#[derive(serde::Serialize)]
struct StreamTicket {
    sub: String,
    purpose: String,
    exp: usize,
    /// The logs this ticket authorises: a site's domain, or `@system` for the
    /// admin-only host logs. It rides INSIDE the signature exactly as the
    /// terminal ticket's `scope` does, and it is the fix for the same
    /// escalation the terminal had: the agent used to read the scope from the
    /// `?domain=` query param AND the log type from `?type=`, both
    /// browser-controlled, so a site-scoped ticket dialled with `type=auth` and
    /// the domain omitted streamed `/var/log/auth.log`. A site owner minting a
    /// legitimate site ticket then read the host's SSH auth log. Binding the
    /// scope into the signature is what closes it; `@system` contains an `@`,
    /// which the agent's domain validation rejects, so it cannot collide with a
    /// real domain.
    scope: String,
}

/// GET /api/logs — System-wide logs (admin only).
pub async fn system_logs(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Query(q): Query<LogQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let log_type = q.log_type.as_deref().unwrap_or("nginx_access");
    if !["nginx_access", "nginx_error", "syslog", "auth", "php_fpm"].contains(&log_type) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid log type"));
    }
    let lines = q.lines.unwrap_or(100).max(1).min(1000);
    let mut agent_path = format!("/logs?type={}&lines={}", log_type, lines);
    if let Some(ref filter) = q.filter {
        agent_path.push_str(&format!("&filter={}", urlencoding::encode(filter)));
    }

    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("System logs", e))?;

    Ok(Json(result))
}

/// GET /api/sites/{id}/logs — Site-specific logs.
pub async fn site_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<uuid::Uuid>,
    Query(q): Query<LogQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Asking the wrong host for a site's logs does not fail. That machine has no
    // vhost by this name, so it answers with nothing, and nothing is exactly what a
    // quiet site looks like — the operator reads "no traffic" instead of "wrong
    // question". The row names the host.
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let log_type = q.log_type.as_deref().unwrap_or("access");
    if !["access", "error"].contains(&log_type) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid site log type"));
    }
    let lines = q.lines.unwrap_or(100).max(1).min(1000);
    let mut agent_path = format!("/logs/{}?type={}&lines={}", domain, log_type, lines);
    if let Some(ref filter) = q.filter {
        agent_path.push_str(&format!("&filter={}", urlencoding::encode(filter)));
    }

    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("Site logs", e))?;

    Ok(Json(result))
}

/// GET /api/logs/search — Search system logs with grep/regex (admin only).
pub async fn search_system_logs(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Query(q): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let log_type = q.log_type.as_deref().unwrap_or("nginx_access");
    if !["nginx_access", "nginx_error", "syslog", "auth", "php_fpm"].contains(&log_type) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid log type"));
    }

    let pattern = q.pattern.as_deref().unwrap_or("");
    if pattern.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Pattern is required"));
    }

    let max = q.max.unwrap_or(500).max(1).min(5000);
    let agent_path = format!(
        "/logs/search?type={}&pattern={}&max={}",
        log_type,
        urlencoding::encode(pattern),
        max
    );

    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("System log search", e))?;

    Ok(Json(result))
}

/// GET /api/sites/{id}/logs/search — Search site logs with grep/regex.
pub async fn search_site_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<uuid::Uuid>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Same as the reader above, and a search is the worse of the two: "no matching
    // lines" is a confident negative answer, and an operator searching a remote
    // site's access log for an attack would have got one.
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let log_type = q.log_type.as_deref().unwrap_or("access");
    let full_type = match log_type {
        "access" => format!("nginx_access:{domain}"),
        "error" => format!("nginx_error:{domain}"),
        _ => return Err(err(StatusCode::BAD_REQUEST, "Invalid site log type")),
    };

    let pattern = q.pattern.as_deref().unwrap_or("");
    if pattern.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Pattern is required"));
    }

    let max = q.max.unwrap_or(500).max(1).min(5000);
    let agent_path = format!(
        "/logs/search?type={}&pattern={}&max={}",
        urlencoding::encode(&full_type),
        urlencoding::encode(pattern),
        max
    );

    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("Site log search", e))?;

    Ok(Json(result))
}

/// GET /api/logs/stream/token — Generate a short-lived JWT for WebSocket log streaming.
pub async fn stream_token(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
    Query(q): Query<StreamTokenQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Answer here rather than minting a ticket the receiving agent must reject.
    crate::helpers::require_local_agent_scope(&state, server_id, "Live log streaming").await?;

    // System-level streaming requires admin
    if q.site_id.is_none() && claims.role != "admin" {
        return Err(err(
            StatusCode::FORBIDDEN,
            "Admin access required for system log streaming",
        ));
    }

    let mut domain: Option<String> = None;

    // Resolve domain from site_id if provided
    if let Some(ref sid) = q.site_id {
        let site_id: uuid::Uuid = sid
            .parse()
            .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid site_id"))?;

        let row: Option<(String,)> =
            sqlx::query_as(&format!("SELECT s.domain FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE))
                .bind(site_id)
                .bind(claims.sub)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| internal_error("stream token", e))?;

        domain = row.map(|(d,)| d);
        if domain.is_none() {
            return Err(err(StatusCode::NOT_FOUND, "Site not found"));
        }
    }

    // The scope is the SIGNED decision, not `?domain=`/`?type=`. A site ticket
    // is bound to its own domain; a system ticket (admin only, checked above)
    // carries the `@system` sentinel so the agent knows the host logs are
    // allowed without trusting any query param.
    let scope = domain.clone().unwrap_or_else(|| "@system".to_string());

    let ticket = StreamTicket {
        sub: claims.email,
        purpose: "log_stream".to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::seconds(60)).timestamp() as usize,
        scope,
    };

    let token = encode(
        &Header::default(),
        &ticket,
        &EncodingKey::from_secret(agent.token().await.as_bytes()),
    )
    .map_err(|e| internal_error("stream token", e))?;

    Ok(Json(serde_json::json!({
        "token": token,
        "domain": domain,
        "type": q.log_type.as_deref().unwrap_or("nginx_access"),
    })))
}

/// GET /api/logs/stats — Log aggregation stats for the server in scope (admin only).
///
/// With no `?domain=` this is the host-wide question its siblings on this page ask,
/// and the answer belongs to the server the operator has selected.
///
/// With one, it used to be neither authorised nor resolved: the parameter went
/// straight into the agent path, so no row was ever read and there was nothing to
/// check the caller against — the browser named the subject and the panel asked it.
/// Being admin-only did not cover that. An administrator here is not a superuser;
/// the shared predicate lets one reach the sites on the machines they operate, and
/// stops at a server somebody else registered. A parameter is now a lookup key and
/// nothing else, and the domain that reaches the agent is the one read back off the
/// row.
///
/// The row read is deliberately pinned to the server in scope, which is what makes
/// this different from the per-site readers above. Two things force it. A domain is
/// only unique PER SERVER — the constraint is `(domain, server_id)` — so a
/// domain-keyed read with no server term is ambiguous the moment the same name is
/// hosted twice, and picking either row would be a guess. And the question itself is
/// host-shaped: these stats are computed by grepping one machine's nginx logs, so a
/// site that lives elsewhere has no answer here to give. It is refused rather than
/// answered with the zeroes a quiet site would produce, and the operator moves the
/// server picker — the same picker every other control on this page already obeys.
/// Pinning also makes a misdispatch impossible by construction: the host that
/// answers and the host the row names cannot come apart.
pub async fn log_stats(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(server_id, agent): ServerScope,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = match params.get("domain") {
        Some(requested) => {
            let row: Option<(uuid::Uuid,)> =
                sqlx::query_as("SELECT id FROM sites WHERE domain = $1 AND server_id = $2")
                    .bind(requested.as_str())
                    .bind(server_id)
                    .fetch_optional(&state.db)
                    .await
                    .map_err(|e| internal_error("log stats", e))?;

            // Identity first, then permission — and the permission half is the one
            // shared decision, not a copy of it living here. The name that goes on
            // the wire is the one it hands back.
            let site_id = row
                .map(|(id,)| id)
                .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
            let resolved =
                crate::helpers::site_domain_for_caller(&state, site_id, &claims).await?;
            format!("?domain={resolved}")
        }
        None => String::new(),
    };

    let result = agent
        .get(&format!("/logs/stats{domain}"))
        .await
        .map_err(|e| agent_error("Log stats", e))?;
    Ok(Json(result))
}

/// GET /api/logs/docker — List managed Docker containers (admin only).
pub async fn docker_log_containers(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .get("/logs/docker")
        .await
        .map_err(|e| agent_error("Docker logs", e))?;
    Ok(Json(result))
}

/// GET /api/logs/docker/{container} — Docker container logs (admin only).
pub async fn docker_log_view(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(container): Path<String>,
    ServerScope(_server_id, agent): ServerScope,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if container.is_empty() || !container.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container name"));
    }
    let lines = params
        .get("lines")
        .unwrap_or(&"200".to_string())
        .clone();
    let result = agent
        .get(&format!("/logs/docker/{container}?lines={lines}"))
        .await
        .map_err(|e| agent_error("Docker logs", e))?;
    Ok(Json(result))
}

/// GET /api/logs/service/{service} — Systemd service logs (admin only).
pub async fn service_logs(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(service): Path<String>,
    ServerScope(_server_id, agent): ServerScope,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if service.is_empty() || !service.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid service name"));
    }
    let lines = params
        .get("lines")
        .unwrap_or(&"100".to_string())
        .clone();
    let result = agent
        .get(&format!("/logs/service/{service}?lines={lines}"))
        .await
        .map_err(|e| agent_error("Service logs", e))?;
    Ok(Json(result))
}

/// GET /api/logs/sizes — Log file sizes (admin only).
pub async fn log_sizes(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .get("/logs/sizes")
        .await
        .map_err(|e| agent_error("Log sizes", e))?;
    Ok(Json(result))
}

/// POST /api/logs/truncate — Truncate a log file (admin only).
pub async fn truncate_log(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent
        .post("/logs/truncate", Some(body))
        .await
        .map_err(|e| agent_error("Truncate", e))?;
    crate::services::activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "logs.truncate",
        None,
        None,
        None,
        None,
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/logs/check-errors — Scan recent logs for error patterns (admin only).
pub async fn check_errors(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get recent nginx error count
    let stats = agent.get("/logs/stats").await.ok();
    let error_5xx = stats
        .as_ref()
        .and_then(|s| s.get("errors_5xx"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let total = stats
        .as_ref()
        .and_then(|s| s.get("requests_total"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1);

    let error_rate = (error_5xx as f64 / total as f64 * 100.0 * 10.0).round() / 10.0;

    Ok(Json(serde_json::json!({
        "error_5xx": error_5xx,
        "total_requests": total,
        "error_rate_percent": error_rate,
        "threshold": "5% error rate with >10 errors",
        "status": if error_rate > 5.0 && error_5xx > 10 { "warning" } else { "ok" },
    })))
}

/// GET /api/system/processes — Top processes (admin only).
pub async fn processes(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .get("/system/processes")
        .await
        .map_err(|e| agent_error("System processes", e))?;

    Ok(Json(result))
}

/// GET /api/system/network — Network I/O stats (admin only).
pub async fn network(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .get("/system/network")
        .await
        .map_err(|e| agent_error("Network stats", e))?;

    Ok(Json(result))
}
