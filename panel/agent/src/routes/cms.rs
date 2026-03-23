use axum::{extract::Path, http::StatusCode, routing::post, Json, Router};

use crate::routes::{is_valid_domain, AppState};
use crate::services::cms;

#[derive(serde::Deserialize)]
struct CmsInstallRequest {
    cms: String,
    title: Option<String>,
    admin_user: Option<String>,
    admin_pass: Option<String>,
    admin_email: Option<String>,
    db_name: Option<String>,
    db_user: Option<String>,
    db_pass: Option<String>,
    db_host: Option<String>,
}

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({"error": msg})))
}

async fn install(
    Path(domain): Path<String>,
    Json(body): Json<CmsInstallRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }

    // Validate db_name and db_user if present
    fn is_valid_db_identifier(s: &str) -> bool {
        !s.is_empty() && s.len() <= 64
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    if let Some(ref db_name) = body.db_name {
        if !is_valid_db_identifier(db_name) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid db_name"));
        }
    }
    if let Some(ref db_user) = body.db_user {
        if !is_valid_db_identifier(db_user) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid db_user"));
        }
    }
    // Validate admin_email contains exactly one @
    if let Some(ref email) = body.admin_email {
        if email.matches('@').count() != 1 {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid admin_email"));
        }
    }
    // Validate db_pass: no newlines or null bytes
    if let Some(ref db_pass) = body.db_pass {
        if db_pass.is_empty() || db_pass.contains('\n') || db_pass.contains('\r') || db_pass.contains('\0') {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid db_pass"));
        }
    }
    // Validate admin_pass: no newlines or null bytes
    if let Some(ref admin_pass) = body.admin_pass {
        if admin_pass.is_empty() || admin_pass.contains('\n') || admin_pass.contains('\r') || admin_pass.contains('\0') {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid admin_pass"));
        }
    }

    let title = body.title.as_deref().unwrap_or("My Site");

    let result = match body.cms.as_str() {
        "laravel" => {
            let db_name = body.db_name.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_name is required"))?;
            let db_user = body.db_user.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_user is required"))?;
            let db_pass = body.db_pass.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_pass is required"))?;
            let db_host = body.db_host.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_host is required"))?;
            cms::install_laravel(&domain, db_name, db_user, db_pass, db_host, title).await
        }
        "drupal" => {
            let db_name = body.db_name.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_name is required"))?;
            let db_user = body.db_user.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_user is required"))?;
            let db_pass = body.db_pass.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_pass is required"))?;
            let db_host = body.db_host.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_host is required"))?;
            let admin_user = body.admin_user.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "admin_user is required"))?;
            let admin_pass = body.admin_pass.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "admin_pass is required"))?;
            let admin_email = body.admin_email.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "admin_email is required"))?;
            cms::install_drupal(&domain, db_name, db_user, db_pass, db_host, title, admin_user, admin_pass, admin_email).await
        }
        "joomla" => {
            let db_name = body.db_name.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_name is required"))?;
            let db_user = body.db_user.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_user is required"))?;
            let db_pass = body.db_pass.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_pass is required"))?;
            let db_host = body.db_host.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_host is required"))?;
            let admin_user = body.admin_user.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "admin_user is required"))?;
            let admin_pass = body.admin_pass.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "admin_pass is required"))?;
            let admin_email = body.admin_email.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "admin_email is required"))?;
            cms::install_joomla(&domain, db_name, db_user, db_pass, db_host, title, admin_user, admin_pass, admin_email).await
        }
        "symfony" => {
            cms::install_symfony(&domain, title).await
        }
        "codeigniter" => {
            let db_name = body.db_name.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_name is required"))?;
            let db_user = body.db_user.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_user is required"))?;
            let db_pass = body.db_pass.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_pass is required"))?;
            let db_host = body.db_host.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "db_host is required"))?;
            cms::install_codeigniter(&domain, db_name, db_user, db_pass, db_host, title).await
        }
        other => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                &format!("Unsupported CMS: {other}. Use: laravel, drupal, joomla, symfony, codeigniter"),
            ));
        }
    };

    result
        .map(|msg| {
            (
                StatusCode::CREATED,
                Json(serde_json::json!({ "ok": true, "message": msg })),
            )
        })
        .map_err(|e| err(StatusCode::UNPROCESSABLE_ENTITY, &e))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/cms/{domain}/install", post(install))
}
