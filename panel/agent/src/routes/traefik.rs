use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};

use crate::routes::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/traefik/install", post(install))
        .route("/traefik/uninstall", post(uninstall))
        .route("/traefik/status", get(status))
        .route("/traefik/route", post(add_route).delete(remove_route))
}

async fn install(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let acme_email = body["acme_email"].as_str().unwrap_or("admin@localhost");

    match crate::services::traefik::install(&state.docker, acme_email).await {
        Ok(status) => Ok(Json(serde_json::to_value(status).unwrap_or_default())),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

async fn uninstall(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    match crate::services::traefik::uninstall(&state.docker).await {
        Ok(()) => Ok(Json(serde_json::json!({ "ok": true }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

async fn status(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let s = crate::services::traefik::status(&state.docker).await;
    Json(serde_json::to_value(s).unwrap_or_default())
}

/// POST /traefik/route — Write a Traefik dynamic route config for an app.
async fn add_route(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let domain = body["domain"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing 'domain'".to_string()))?;
    let port = body["port"]
        .as_u64()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing 'port'".to_string()))? as u16;
    let ssl = body["ssl"].as_bool().unwrap_or(true);

    crate::services::traefik::write_route_config(domain, port, ssl)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "ok": true, "domain": domain, "port": port, "ssl": ssl })))
}

/// DELETE /traefik/route — Remove a Traefik dynamic route config.
async fn remove_route(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let domain = body["domain"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing 'domain'".to_string()))?;

    crate::services::traefik::remove_route_config(domain);

    Ok(Json(serde_json::json!({ "ok": true, "domain": domain })))
}
