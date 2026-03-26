use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, require_admin, ApiError};
use crate::services::activity;
use crate::services::notifications;
use crate::AppState;

#[derive(sqlx::FromRow, serde::Serialize)]
pub struct ScanRow {
    id: uuid::Uuid,
    scan_type: String,
    status: String,
    findings_count: i32,
    critical_count: i32,
    warning_count: i32,
    info_count: i32,
    started_at: chrono::DateTime<chrono::Utc>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(sqlx::FromRow, serde::Serialize)]
pub struct FindingRow {
    id: uuid::Uuid,
    check_type: String,
    severity: String,
    title: String,
    description: Option<String>,
    file_path: Option<String>,
    remediation: Option<String>,
}

/// POST /api/security/scan — Trigger a security scan (admin only).
/// For self-hosted: calls agent directly. For SaaS with server_id: dispatches command.
pub async fn trigger_scan(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    // Create scan record (server_id NULL for local/self-hosted)
    let scan_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO security_scans (scan_type, status) VALUES ('full', 'running') RETURNING id",
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("trigger scan", e))?;

    // Call agent directly
    let result = agent
        .post("/security/scan", None::<serde_json::Value>)
        .await
        .map_err(|e| agent_error("Security scan", e))?;

    // Process findings from agent response
    let findings = result["findings"].as_array();
    let file_hashes = result["file_hashes"].as_array();

    let mut critical = 0i32;
    let mut warning = 0i32;
    let mut info = 0i32;

    if let Some(findings) = findings {
        for f in findings {
            let severity = f["severity"].as_str().unwrap_or("info");
            match severity {
                "critical" => critical += 1,
                "warning" => warning += 1,
                _ => info += 1,
            }

            if let Err(e) = sqlx::query(
                "INSERT INTO security_findings (scan_id, check_type, severity, title, description, file_path, remediation) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(scan_id)
            .bind(f["check_type"].as_str().unwrap_or(""))
            .bind(severity)
            .bind(f["title"].as_str().unwrap_or(""))
            .bind(f["description"].as_str())
            .bind(f["file_path"].as_str())
            .bind(f["remediation"].as_str())
            .execute(&state.db)
            .await {
                tracing::error!("Failed to insert security finding: {e}");
            }
        }
    }

    let total = critical + warning + info;

    // Update scan record
    if let Err(e) = sqlx::query(
        "UPDATE security_scans SET status = 'completed', completed_at = NOW(), \
         findings_count = $1, critical_count = $2, warning_count = $3, info_count = $4 \
         WHERE id = $5",
    )
    .bind(total)
    .bind(critical)
    .bind(warning)
    .bind(info)
    .bind(scan_id)
    .execute(&state.db)
    .await {
        tracing::error!("Failed to update scan status: {e}");
    }

    // Store file integrity baselines
    if let Some(hashes) = file_hashes {
        for h in hashes {
            let path = h["path"].as_str().unwrap_or("");
            let hash = h["hash"].as_str().unwrap_or("");
            let size = h["size"].as_i64().unwrap_or(0);

            // Check if hash changed (potential integrity violation)
            let existing: Option<(String,)> = match sqlx::query_as(
                "SELECT sha256_hash FROM file_integrity_baselines \
                 WHERE server_id IS NULL AND file_path = $1",
            )
            .bind(path)
            .fetch_optional(&state.db)
            .await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("Failed to check file integrity baseline for {path}: {e}");
                    None
                }
            };

            if let Some((old_hash,)) = &existing {
                if old_hash != hash {
                    // File changed — add a finding
                    if let Err(e) = sqlx::query(
                        "INSERT INTO security_findings (scan_id, check_type, severity, title, description, file_path, remediation) \
                         VALUES ($1, 'file_integrity', 'warning', $2, $3, $4, 'Verify this change was intentional')",
                    )
                    .bind(scan_id)
                    .bind(format!("File modified: {path}"))
                    .bind(format!("Hash changed from {old_hash} to {hash}"))
                    .bind(path)
                    .execute(&state.db)
                    .await {
                        tracing::error!("Failed to insert file integrity finding: {e}");
                    }

                    warning += 1;

                    // Update counts
                    if let Err(e) = sqlx::query(
                        "UPDATE security_scans SET findings_count = $1, warning_count = $2 WHERE id = $3",
                    )
                    .bind(critical + warning + info)
                    .bind(warning)
                    .bind(scan_id)
                    .execute(&state.db)
                    .await {
                        tracing::error!("Failed to update scan counts: {e}");
                    }
                }
            }

            // Upsert baseline
            if let Err(e) = sqlx::query(
                "INSERT INTO file_integrity_baselines (file_path, sha256_hash, file_size) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (server_id, file_path) DO UPDATE SET sha256_hash = $2, file_size = $3, updated_at = NOW()",
            )
            .bind(path)
            .bind(hash)
            .bind(size)
            .execute(&state.db)
            .await {
                tracing::error!("Failed to upsert file integrity baseline: {e}");
            }
        }
    }

    let detail = format!("{total} findings, {critical} critical");
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "security.scan",
        Some("security"),
        None,
        Some(&detail),
        None,
    )
    .await;

    // Create alerts for critical/warning findings
    if critical > 0 || warning > 0 {
        let severity = if critical > 0 { "critical" } else { "warning" };
        let alert_title = format!(
            "Security scan: {} critical, {} warning findings",
            critical, warning
        );
        let alert_message = format!(
            "A security scan completed with {} total findings ({} critical, {} warning). \
             Review the scan results in the Security section.",
            total, critical, warning
        );

        notifications::fire_alert(
            &state.db,
            claims.sub,
            None,
            None,
            "security",
            severity,
            &alert_title,
            &alert_message,
        )
        .await;
    }

    Ok(Json(serde_json::json!({
        "id": scan_id,
        "findings_count": critical + warning + info,
        "critical_count": critical,
        "warning_count": warning,
        "info_count": info,
    })))
}

/// GET /api/security/scans — List recent security scans (admin only).
pub async fn list_scans(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<ScanRow>>, ApiError> {
    require_admin(&claims.role)?;

    let scans: Vec<ScanRow> = sqlx::query_as(
        "SELECT id, scan_type, status, findings_count, critical_count, warning_count, info_count, \
         started_at, completed_at FROM security_scans \
         WHERE server_id IS NULL \
         ORDER BY created_at DESC LIMIT 20",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list scans", e))?;

    Ok(Json(scans))
}

/// GET /api/security/scans/{id} — Get scan details with findings (admin only).
pub async fn get_scan(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(scan_id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let scan: ScanRow = sqlx::query_as(
        "SELECT id, scan_type, status, findings_count, critical_count, warning_count, info_count, \
         started_at, completed_at FROM security_scans WHERE id = $1",
    )
    .bind(scan_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("get scan", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Scan not found"))?;

    let findings: Vec<FindingRow> = sqlx::query_as(
        "SELECT id, check_type, severity, title, description, file_path, remediation \
         FROM security_findings WHERE scan_id = $1 ORDER BY \
         CASE severity WHEN 'critical' THEN 0 WHEN 'warning' THEN 1 ELSE 2 END, title",
    )
    .bind(scan_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("get scan", e))?;

    Ok(Json(serde_json::json!({
        "scan": scan,
        "findings": findings,
    })))
}

/// GET /api/security/posture — Overall security posture summary (admin only).
pub async fn posture(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    // Get latest scan
    let latest: Option<ScanRow> = sqlx::query_as(
        "SELECT id, scan_type, status, findings_count, critical_count, warning_count, info_count, \
         started_at, completed_at FROM security_scans \
         WHERE server_id IS NULL AND status = 'completed' \
         ORDER BY completed_at DESC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("posture", e))?;

    // Get total scans count
    let (total_scans,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM security_scans WHERE server_id IS NULL",
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("posture", e))?;

    // Calculate score: 100 base, -20 per critical, -5 per warning
    let score = if let Some(ref scan) = latest {
        (100 - scan.critical_count * 20 - scan.warning_count * 5).max(0)
    } else {
        -1 // No scan yet
    };

    Ok(Json(serde_json::json!({
        "score": score,
        "total_scans": total_scans,
        "latest_scan": latest,
    })))
}
