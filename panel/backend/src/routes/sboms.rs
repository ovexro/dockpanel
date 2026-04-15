// Per-image SBOM (SPDX 2.3 JSON) surface.
//
// Companion to image_scans: that module reports vulnerabilities (what is
// broken); this module reports composition (what is installed). Operators
// want both — the SBOM is the supply-chain artifact, the vuln scan is the
// risk assessment. The two share the same agent install pattern but live in
// distinct backend modules so the UI can wire them independently.

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::auth::{AuthUser, ServerScope};
use crate::error::{agent_error, err, internal_error, require_admin, ApiError};
use crate::AppState;

#[derive(Serialize)]
pub struct SbomSettings {
    pub installed: bool,
}

/// GET /api/sbom/settings — Whether the syft binary is installed on the agent.
pub async fn get_settings(
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<SbomSettings>, ApiError> {
    require_admin(&claims.role)?;

    let installed = agent
        .get("/sbom/status")
        .await
        .ok()
        .and_then(|v| v.get("installed").and_then(|b| b.as_bool()))
        .unwrap_or(false);

    Ok(Json(SbomSettings { installed }))
}

/// POST /api/sbom/install — Install syft on the agent.
pub async fn install_scanner(
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent
        .post_long("/sbom/install", None::<serde_json::Value>, 300)
        .await
        .map_err(|e| agent_error("install syft", e))?;
    Ok(Json(result))
}

/// POST /api/sbom/uninstall — Remove syft on the agent.
pub async fn uninstall_scanner(
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent
        .post("/sbom/uninstall", None::<serde_json::Value>)
        .await
        .map_err(|e| agent_error("uninstall syft", e))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct GenerateRequest {
    pub image: String,
}

/// POST /api/sbom/generate — Generate an SBOM for an arbitrary image.
pub async fn generate(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<GenerateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let spdx = generate_and_store(&state.db, &agent, &body.image).await?;
    Ok(Json(serde_json::json!({
        "image": body.image,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "spdx": spdx,
    })))
}

/// POST /api/apps/{name}/sbom — Generate an SBOM for a specific app's image.
pub async fn generate_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let image = resolve_app_image(&agent, &name)
        .await?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "App not found or has no image"))?;
    let spdx = generate_and_store(&state.db, &agent, &image).await?;
    Ok(Json(serde_json::json!({
        "image": image,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "spdx": spdx,
    })))
}

/// GET /api/apps/{name}/sbom — Latest stored SBOM for the app's image as
/// SPDX JSON download (Content-Disposition: attachment).
pub async fn download_app_sbom(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    require_admin(&claims.role)?;
    let image = resolve_app_image(&agent, &name)
        .await?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "App not found or has no image"))?;

    let row: Option<(serde_json::Value,)> =
        sqlx::query_as("SELECT spdx FROM image_sbom WHERE image = $1")
            .bind(&image)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("fetch sbom", e))?;

    let spdx = row
        .map(|r| r.0)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "No SBOM generated for this image yet"))?;

    let body = serde_json::to_vec_pretty(&spdx)
        .map_err(|e| internal_error("serialize sbom", e))?;
    let safe_name = sanitize_filename(&name);
    let filename = format!("{safe_name}.spdx.json");

    let resp = (
        [
            (header::CONTENT_TYPE, "application/json"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response();
    Ok(resp)
}

/// GET /api/sbom/image/{ref} — Latest stored SBOM for an arbitrary image
/// reference. Inline (Content-Type application/json) for API consumers.
pub async fn get_image_sbom(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(image): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let row: Option<(serde_json::Value, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT spdx, generated_at FROM image_sbom WHERE image = $1",
    )
    .bind(&image)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("fetch sbom", e))?;

    let (spdx, generated_at) = row
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "No SBOM stored for this image"))?;

    Ok(Json(serde_json::json!({
        "image": image,
        "generated_at": generated_at.to_rfc3339(),
        "spdx": spdx,
    })))
}

// ── Internal helpers ────────────────────────────────────────────────────

/// Disallow path separators and control characters in the download filename.
/// Apps already validate names but defence-in-depth keeps Content-Disposition
/// safe even if the upstream constraint changes.
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

async fn resolve_app_image(
    agent: &crate::services::agent::AgentHandle,
    app_name: &str,
) -> Result<Option<String>, ApiError> {
    let apps = agent
        .get("/apps")
        .await
        .map_err(|e| agent_error("list apps", e))?;
    let arr = match apps.as_array() {
        Some(a) => a,
        None => return Ok(None),
    };
    for a in arr {
        let n = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if n == app_name {
            return Ok(a
                .get("image")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()));
        }
    }
    Ok(None)
}

async fn generate_and_store(
    pool: &sqlx::PgPool,
    agent: &crate::services::agent::AgentHandle,
    image: &str,
) -> Result<serde_json::Value, ApiError> {
    let response = agent
        .post_long(
            "/sbom/scan",
            Some(serde_json::json!({ "image": image })),
            240,
        )
        .await
        .map_err(|e| agent_error("generate sbom", e))?;

    let spdx = response
        .get("spdx")
        .cloned()
        .ok_or_else(|| internal_error("agent returned no spdx field", "missing field"))?;

    sqlx::query(
        "INSERT INTO image_sbom (image, format, spdx, generated_at) \
         VALUES ($1, 'spdx-json', $2, NOW()) \
         ON CONFLICT (image) DO UPDATE \
            SET spdx = EXCLUDED.spdx, generated_at = NOW()",
    )
    .bind(image)
    .bind(&spdx)
    .execute(pool)
    .await
    .map_err(|e| internal_error("store sbom", e))?;

    Ok(spdx)
}
