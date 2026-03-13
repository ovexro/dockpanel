use axum::{
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;

use super::AppState;
use crate::services::smtp;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Deserialize)]
pub struct ConfigureRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
    pub from_name: Option<String>,
    pub encryption: Option<String>,
}

#[derive(Deserialize)]
pub struct TestRequest {
    pub to: String,
    pub from: String,
    pub from_name: Option<String>,
}

/// POST /smtp/configure — Write msmtp config and PHP sendmail path.
async fn configure(
    Json(body): Json<ConfigureRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.host.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Host is required"));
    }

    let from_name = body.from_name.as_deref().unwrap_or("DockPanel");
    let encryption = body.encryption.as_deref().unwrap_or("starttls");

    smtp::configure(
        &body.host,
        body.port,
        &body.username,
        &body.password,
        &body.from,
        from_name,
        encryption,
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /smtp/test — Send a test email.
async fn test_email(
    Json(body): Json<TestRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.to.is_empty() || !body.to.contains('@') {
        return Err(err(StatusCode::BAD_REQUEST, "Valid email address required"));
    }

    let from_name = body.from_name.as_deref().unwrap_or("DockPanel");

    let message = smtp::send_test(&body.to, &body.from, from_name)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "ok": true, "message": message })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/smtp/configure", post(configure))
        .route("/smtp/test", post(test_email))
}
