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
