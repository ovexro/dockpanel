// Per-image vulnerability scanning surface.
//
// Companion to security_scans (full-server scan); this module manages
// per-image scan results so the Apps page can badge individual containers
// and the deploy path can gate on a configurable severity threshold.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::auth::{AuthUser, ServerScope};
use crate::error::{agent_error, err, internal_error, require_admin, ApiError};
use crate::AppState;

#[derive(Serialize, Deserialize, Clone)]
pub struct Vuln {
    pub cve: String,
    pub severity: String,
    pub package: String,
    pub installed_version: String,
    pub fixed_version: Option<String>,
    pub description: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ImageScanResult {
    pub image: String,
    pub scanner: String,
    pub critical_count: u32,
    pub high_count: u32,
    pub medium_count: u32,
    pub low_count: u32,
    pub unknown_count: u32,
    pub vulnerabilities: Vec<Vuln>,
    pub scanned_at: String,
}

#[derive(Serialize)]
pub struct ScanFindingRow {
    pub image: String,
    pub scanner: String,
    pub critical_count: i32,
    pub high_count: i32,
    pub medium_count: i32,
    pub low_count: i32,
    pub unknown_count: i32,
    pub vulnerabilities: serde_json::Value,
    pub scanned_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
pub struct ScanSettings {
    pub enabled: bool,
    pub on_deploy: bool,
    pub deploy_gate: String,
    pub interval_hours: i32,
    pub installed: bool,
}

#[derive(Deserialize)]
pub struct UpdateSettings {
    pub enabled: bool,
    pub on_deploy: bool,
    pub deploy_gate: String,
    pub interval_hours: i32,
}

// ── Settings helpers ────────────────────────────────────────────────────

pub async fn read_settings(pool: &sqlx::PgPool) -> Result<(bool, bool, String, i32), sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN \
         ('image_scan_enabled', 'image_scan_on_deploy', 'image_scan_deploy_gate', 'image_scan_interval_hours')"
    )
    .fetch_all(pool)
    .await?;

    let mut enabled = false;
    let mut on_deploy = false;
    let mut gate = "none".to_string();
    let mut hours = 24i32;
    for (k, v) in rows {
        match k.as_str() {
            "image_scan_enabled" => enabled = v == "true",
            "image_scan_on_deploy" => on_deploy = v == "true",
            "image_scan_deploy_gate" => gate = v,
            "image_scan_interval_hours" => hours = v.parse().unwrap_or(24),
            _ => {}
        }
    }
    Ok((enabled, on_deploy, gate, hours))
}

fn valid_gate(g: &str) -> bool {
    matches!(g, "none" | "critical" | "high" | "medium")
}

/// True if the result exceeds the configured deploy-gate threshold.
pub fn exceeds_threshold(gate: &str, r: &ImageScanResult) -> bool {
    match gate {
        "critical" => r.critical_count > 0,
        "high" => r.critical_count > 0 || r.high_count > 0,
        "medium" => r.critical_count > 0 || r.high_count > 0 || r.medium_count > 0,
        _ => false,
    }
}

// ── Routes ──────────────────────────────────────────────────────────────

/// GET /api/image-scan/settings
pub async fn get_settings(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<ScanSettings>, ApiError> {
    require_admin(&claims.role)?;

    let (enabled, on_deploy, gate, hours) = read_settings(&state.db)
        .await
        .map_err(|e| internal_error("read image scan settings", e))?;

    let installed = agent
        .get("/image-scan/status")
        .await
        .ok()
        .and_then(|v| v.get("installed").and_then(|b| b.as_bool()))
        .unwrap_or(false);

    Ok(Json(ScanSettings {
        enabled,
        on_deploy,
        deploy_gate: gate,
        interval_hours: hours,
        installed,
    }))
}

/// PUT /api/image-scan/settings
pub async fn update_settings(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<UpdateSettings>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    if !valid_gate(&body.deploy_gate) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid deploy_gate (none|critical|high|medium)"));
    }
    if !(1..=720).contains(&body.interval_hours) {
        return Err(err(StatusCode::BAD_REQUEST, "interval_hours must be 1..=720"));
    }

    for (key, value) in [
        ("image_scan_enabled", if body.enabled { "true" } else { "false" }),
        ("image_scan_on_deploy", if body.on_deploy { "true" } else { "false" }),
        ("image_scan_deploy_gate", body.deploy_gate.as_str()),
    ] {
        sqlx::query("INSERT INTO settings (key, value) VALUES ($1, $2) \
                     ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
            .bind(key)
            .bind(value)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("save image scan setting", e))?;
    }
    sqlx::query("INSERT INTO settings (key, value) VALUES ('image_scan_interval_hours', $1) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
        .bind(body.interval_hours.to_string())
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("save image scan interval", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/image-scan/install
pub async fn install_scanner(
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent
        .post_long("/image-scan/install", None::<serde_json::Value>, 300)
        .await
        .map_err(|e| agent_error("install image scanner", e))?;
    Ok(Json(result))
}

/// POST /api/image-scan/uninstall
pub async fn uninstall_scanner(
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent
        .post("/image-scan/uninstall", None::<serde_json::Value>)
        .await
        .map_err(|e| agent_error("uninstall image scanner", e))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct ScanByImageRequest {
    pub image: String,
}

/// POST /api/image-scan/scan — Scan an arbitrary image (ad-hoc).
pub async fn scan_image(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<ScanByImageRequest>,
) -> Result<Json<ImageScanResult>, ApiError> {
    require_admin(&claims.role)?;
    let result = scan_and_store(&state.db, &agent, &body.image).await?;
    Ok(Json(result))
}

/// POST /api/apps/{name}/scan — Scan the image used by a specific Docker app.
pub async fn scan_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(name): Path<String>,
) -> Result<Json<ImageScanResult>, ApiError> {
    require_admin(&claims.role)?;

    let image = resolve_app_image(&agent, &name)
        .await?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "App not found or has no image"))?;

    let result = scan_and_store(&state.db, &agent, &image).await?;
    Ok(Json(result))
}

/// GET /api/apps/{name}/scan — Latest stored scan result for the app's image.
pub async fn get_app_scan(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(name): Path<String>,
) -> Result<Json<Option<ScanFindingRow>>, ApiError> {
    require_admin(&claims.role)?;

    let image = match resolve_app_image(&agent, &name).await? {
        Some(i) => i,
        None => return Ok(Json(None)),
    };

    let row: Option<(String, String, i32, i32, i32, i32, i32, serde_json::Value, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT image, scanner, critical_count, high_count, medium_count, low_count, \
             unknown_count, vulnerabilities, scanned_at \
             FROM image_scan_findings \
             WHERE image = $1 \
             ORDER BY scanned_at DESC LIMIT 1",
        )
        .bind(&image)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("fetch image scan", e))?;

    Ok(Json(row.map(row_to_finding)))
}

/// GET /api/image-scan/recent — Latest result for every scanned image.
pub async fn list_recent(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<ScanFindingRow>>, ApiError> {
    require_admin(&claims.role)?;

    let rows: Vec<(String, String, i32, i32, i32, i32, i32, serde_json::Value, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT DISTINCT ON (image) \
                image, scanner, critical_count, high_count, medium_count, low_count, \
                unknown_count, vulnerabilities, scanned_at \
             FROM image_scan_findings \
             ORDER BY image, scanned_at DESC",
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list image scans", e))?;

    Ok(Json(rows.into_iter().map(row_to_finding).collect()))
}

fn row_to_finding(
    r: (String, String, i32, i32, i32, i32, i32, serde_json::Value, chrono::DateTime<chrono::Utc>),
) -> ScanFindingRow {
    ScanFindingRow {
        image: r.0,
        scanner: r.1,
        critical_count: r.2,
        high_count: r.3,
        medium_count: r.4,
        low_count: r.5,
        unknown_count: r.6,
        vulnerabilities: r.7,
        scanned_at: r.8,
    }
}

// ── Deploy preflight gate ───────────────────────────────────────────────

/// Soft deploy gate: if a recent scan already shows the template's image
/// exceeds the configured threshold, refuse the deploy. If no recent scan
/// exists, allow the deploy and trigger a background scan so the next deploy
/// of the same image enforces the gate. This avoids blocking deploys for
/// 30-180s on first encounter while still hardening the steady state.
pub async fn preflight_gate(
    pool: &sqlx::PgPool,
    agent: &crate::services::agent::AgentHandle,
    template_id: &str,
) -> Result<(), ApiError> {
    let (enabled, on_deploy, gate, _hours) = read_settings(pool)
        .await
        .map_err(|e| internal_error("read scan settings for gate", e))?;
    if !enabled || !on_deploy || gate == "none" {
        return Ok(());
    }

    let image = match resolve_template_image(agent, template_id).await {
        Some(i) => i,
        None => return Ok(()), // unknown template, let deploy fail naturally
    };

    // Look up the most recent scan within 7 days
    let recent: Option<(i32, i32, i32, i32, i32)> = sqlx::query_as(
        "SELECT critical_count, high_count, medium_count, low_count, unknown_count \
         FROM image_scan_findings \
         WHERE image = $1 AND scanned_at > NOW() - INTERVAL '7 days' \
         ORDER BY scanned_at DESC LIMIT 1",
    )
    .bind(&image)
    .fetch_optional(pool)
    .await
    .map_err(|e| internal_error("read recent scan", e))?;

    match recent {
        Some((critical, high, medium, low, unknown)) => {
            let result = ImageScanResult {
                image: image.clone(),
                scanner: "grype".into(),
                critical_count: critical as u32,
                high_count: high as u32,
                medium_count: medium as u32,
                low_count: low as u32,
                unknown_count: unknown as u32,
                vulnerabilities: vec![],
                scanned_at: "".into(),
            };
            if exceeds_threshold(&gate, &result) {
                return Err(err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!(
                        "Deploy blocked by image scan gate ({gate}): {} critical / {} high / {} medium in {}",
                        critical, high, medium, image
                    ),
                ));
            }
            Ok(())
        }
        None => {
            // Best-effort background scan so the next attempt is gated.
            let pool_clone = pool.clone();
            let agent_clone = agent.clone();
            let img = image.clone();
            tokio::spawn(async move {
                if let Err(e) = scan_and_store(&pool_clone, &agent_clone, &img).await {
                    tracing::warn!("Background image scan failed for {img}: {e:?}");
                }
            });
            Ok(())
        }
    }
}

/// Look up a template's image string from the agent's template list.
async fn resolve_template_image(
    agent: &crate::services::agent::AgentHandle,
    template_id: &str,
) -> Option<String> {
    let templates = agent.get("/apps/templates").await.ok()?;
    let arr = templates.as_array()?;
    for t in arr {
        if t.get("id").and_then(|v| v.as_str()) == Some(template_id) {
            return t
                .get("image")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }
    None
}

// ── Internal helpers ────────────────────────────────────────────────────

/// Look up the Docker image for a running DockPanel-managed app by name.
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

/// Run a scan via the agent and persist the result. Public so the deploy
/// gate and the background scheduler can call it.
pub async fn scan_and_store(
    pool: &sqlx::PgPool,
    agent: &crate::services::agent::AgentHandle,
    image: &str,
) -> Result<ImageScanResult, ApiError> {
    let response = agent
        .post_long(
            "/image-scan/scan",
            Some(serde_json::json!({ "image": image })),
            240,
        )
        .await
        .map_err(|e| agent_error("scan image", e))?;

    let result: ImageScanResult = serde_json::from_value(response)
        .map_err(|e| internal_error("parse scan response", e))?;

    let vuln_json = serde_json::to_value(&result.vulnerabilities)
        .map_err(|e| internal_error("serialize vulns", e))?;

    sqlx::query(
        "INSERT INTO image_scan_findings \
         (image, scanner, critical_count, high_count, medium_count, low_count, unknown_count, vulnerabilities) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&result.image)
    .bind(&result.scanner)
    .bind(result.critical_count as i32)
    .bind(result.high_count as i32)
    .bind(result.medium_count as i32)
    .bind(result.low_count as i32)
    .bind(result.unknown_count as i32)
    .bind(&vuln_json)
    .execute(pool)
    .await
    .map_err(|e| internal_error("store image scan", e))?;

    // Trim history per image to keep the table lean — keep last 30 entries.
    sqlx::query(
        "DELETE FROM image_scan_findings WHERE image = $1 AND id NOT IN \
         (SELECT id FROM image_scan_findings WHERE image = $1 ORDER BY scanned_at DESC LIMIT 30)",
    )
    .bind(&result.image)
    .execute(pool)
    .await
    .ok();

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(c: u32, h: u32, m: u32) -> ImageScanResult {
        ImageScanResult {
            image: "x".into(),
            scanner: "grype".into(),
            critical_count: c,
            high_count: h,
            medium_count: m,
            low_count: 0,
            unknown_count: 0,
            vulnerabilities: vec![],
            scanned_at: "now".into(),
        }
    }

    #[test]
    fn gate_thresholds() {
        let clean = r(0, 0, 0);
        let med = r(0, 0, 1);
        let high = r(0, 1, 0);
        let crit = r(1, 0, 0);

        assert!(!exceeds_threshold("none", &crit));
        assert!(exceeds_threshold("critical", &crit));
        assert!(!exceeds_threshold("critical", &high));
        assert!(exceeds_threshold("high", &high));
        assert!(exceeds_threshold("high", &crit));
        assert!(!exceeds_threshold("high", &med));
        assert!(exceeds_threshold("medium", &med));
        assert!(!exceeds_threshold("medium", &clean));
    }

    #[test]
    fn validates_gate() {
        assert!(valid_gate("none"));
        assert!(valid_gate("critical"));
        assert!(valid_gate("high"));
        assert!(valid_gate("medium"));
        assert!(!valid_gate("low"));
        assert!(!valid_gate(""));
        assert!(!valid_gate("bogus"));
    }
}
