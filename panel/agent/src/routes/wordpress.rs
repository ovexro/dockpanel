use axum::{
    extract::Path,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};

use crate::routes::{is_valid_domain, AppState};
use crate::services::wordpress;

#[derive(serde::Deserialize)]
pub struct InstallRequest {
    url: String,
    title: String,
    admin_user: String,
    admin_pass: String,
    admin_email: String,
    db_name: String,
    db_user: String,
    db_pass: String,
    db_host: String,
}

#[derive(serde::Deserialize)]
pub struct NameRequest {
    name: String,
}

#[derive(serde::Deserialize)]
pub struct AutoUpdateRequest {
    enabled: bool,
}

fn validate_domain(domain: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid domain"})),
        ));
    }
    Ok(())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/wordpress/{domain}/detect", get(detect))
        .route("/wordpress/{domain}/info", get(info))
        .route("/wordpress/{domain}/plugins", get(plugins))
        .route("/wordpress/{domain}/themes", get(themes))
        .route("/wordpress/{domain}/install", post(install))
        .route(
            "/wordpress/{domain}/update/{target}",
            post(update),
        )
        .route(
            "/wordpress/{domain}/plugin/{action}",
            post(plugin_action),
        )
        .route(
            "/wordpress/{domain}/theme/{action}",
            post(theme_action),
        )
        .route(
            "/wordpress/{domain}/auto-update",
            get(get_auto_update).post(set_auto_update),
        )
        .route("/wordpress/{domain}/vuln-scan", post(vuln_scan))
        .route("/wordpress/{domain}/security-check", get(security_check))
        .route("/wordpress/{domain}/harden", post(harden))
        .route("/wordpress/{domain}/update-with-rollback", post(update_with_rollback))
}

async fn detect(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    Ok(Json(serde_json::json!({
        "detected": wordpress::detect(&domain),
    })))
}

async fn info(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    if !wordpress::detect(&domain) {
        return Ok(Json(serde_json::json!({ "installed": false })));
    }
    wordpress::info(&domain).await.map(Json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        )
    })
}

async fn plugins(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    wordpress::plugins(&domain).await.map(Json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        )
    })
}

async fn themes(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    wordpress::themes(&domain).await.map(Json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        )
    })
}

async fn install(
    Path(domain): Path<String>,
    Json(body): Json<InstallRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    wordpress::install(
        &domain,
        &body.url,
        &body.title,
        &body.admin_user,
        &body.admin_pass,
        &body.admin_email,
        &body.db_name,
        &body.db_user,
        &body.db_pass,
        &body.db_host,
    )
    .await
    .map(|msg| {
        (
            StatusCode::CREATED,
            Json(serde_json::json!({ "ok": true, "message": msg })),
        )
    })
    .map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": e})),
        )
    })
}

async fn update(
    Path((domain, target)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    let result = match target.as_str() {
        "core" => wordpress::update_core(&domain).await,
        "plugins" => wordpress::update_all_plugins(&domain).await,
        "themes" => wordpress::update_all_themes(&domain).await,
        _ => Err("Invalid target. Use: core, plugins, themes".into()),
    };
    result
        .map(|msg| Json(serde_json::json!({ "ok": true, "message": msg })))
        .map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"error": e})),
            )
        })
}

async fn plugin_action(
    Path((domain, action)): Path<(String, String)>,
    Json(body): Json<NameRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    wordpress::plugin_action(&domain, &body.name, &action)
        .await
        .map(|msg| Json(serde_json::json!({ "ok": true, "message": msg })))
        .map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"error": e})),
            )
        })
}

async fn theme_action(
    Path((domain, action)): Path<(String, String)>,
    Json(body): Json<NameRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    wordpress::theme_action(&domain, &body.name, &action)
        .await
        .map(|msg| Json(serde_json::json!({ "ok": true, "message": msg })))
        .map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"error": e})),
            )
        })
}

async fn get_auto_update(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    Ok(Json(serde_json::json!({
        "enabled": wordpress::is_auto_update_enabled(&domain),
    })))
}

async fn set_auto_update(
    Path(domain): Path<String>,
    Json(body): Json<AutoUpdateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    wordpress::set_auto_update(&domain, body.enabled)
        .await
        .map(|_| Json(serde_json::json!({ "ok": true })))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e})),
            )
        })
}

/// POST /wordpress/{domain}/vuln-scan — Scan for plugin vulnerabilities
async fn vuln_scan(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    crate::services::wp_vulnerability::scan_site(&domain)
        .await
        .map(|result| Json(serde_json::to_value(result).unwrap_or_default()))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e})),
            )
        })
}

/// GET /wordpress/{domain}/security-check — Check security hardening status
async fn security_check(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    crate::services::wp_vulnerability::check_security(&domain)
        .await
        .map(|checks| Json(serde_json::to_value(checks).unwrap_or_default()))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e})),
            )
        })
}

/// POST /wordpress/{domain}/harden — Apply security hardening fixes
async fn harden(
    Path(domain): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    let fixes: Vec<String> = body["fixes"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    if fixes.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "No fixes specified"})),
        ));
    }
    crate::services::wp_vulnerability::apply_hardening(&domain, &fixes)
        .await
        .map(|results| Json(serde_json::to_value(results).unwrap_or_default()))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e})),
            )
        })
}

/// POST /wordpress/{domain}/update-with-rollback — Update WP with snapshot + auto-rollback.
async fn update_with_rollback(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_domain(&domain)?;
    wordpress::update_with_rollback(&domain)
        .await
        .map(Json)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e})),
            )
        })
}
