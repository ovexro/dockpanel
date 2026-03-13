use sqlx::PgPool;
use std::time::Duration;

use crate::services::agent::AgentClient;

/// Background task: runs weekly security scans automatically.
pub async fn run(pool: PgPool, agent: AgentClient) {
    tracing::info!("Security scanner background task started (weekly)");

    // Initial delay: 1 hour after startup
    tokio::time::sleep(Duration::from_secs(3600)).await;

    loop {
        // Check if a scan was done in the last 7 days
        let recent: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM security_scans \
             WHERE server_id IS NULL AND created_at > NOW() - INTERVAL '7 days'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap_or(None);

        let needs_scan = recent.map(|(c,)| c == 0).unwrap_or(true);

        if needs_scan {
            tracing::info!("Running scheduled weekly security scan");
            run_scan(&pool, &agent).await;
        }

        // Check every 6 hours if a weekly scan is due
        tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
    }
}

async fn run_scan(pool: &PgPool, agent: &AgentClient) {
    // Create scan record
    let scan_id: uuid::Uuid = match sqlx::query_scalar(
        "INSERT INTO security_scans (scan_type, status) VALUES ('full', 'running') RETURNING id",
    )
    .fetch_one(pool)
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("Failed to create scan record: {e}");
            return;
        }
    };

    // Call agent
    let result = match agent.post("/security/scan", None::<serde_json::Value>).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Security scan failed: {e}");
            let _ = sqlx::query(
                "UPDATE security_scans SET status = 'failed', completed_at = NOW() WHERE id = $1",
            )
            .bind(scan_id)
            .execute(pool)
            .await;
            return;
        }
    };

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

            let _ = sqlx::query(
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
            .execute(pool)
            .await;
        }
    }

    // File integrity check against baselines
    if let Some(hashes) = file_hashes {
        for h in hashes {
            let path = h["path"].as_str().unwrap_or("");
            let hash = h["hash"].as_str().unwrap_or("");
            let size = h["size"].as_i64().unwrap_or(0);

            let existing: Option<(String,)> = sqlx::query_as(
                "SELECT sha256_hash FROM file_integrity_baselines \
                 WHERE server_id IS NULL AND file_path = $1",
            )
            .bind(path)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

            if let Some((old_hash,)) = &existing {
                if old_hash != hash {
                    let _ = sqlx::query(
                        "INSERT INTO security_findings (scan_id, check_type, severity, title, description, file_path, remediation) \
                         VALUES ($1, 'file_integrity', 'warning', $2, $3, $4, 'Verify this change was intentional')",
                    )
                    .bind(scan_id)
                    .bind(format!("File modified: {path}"))
                    .bind(format!("Hash changed from {old_hash} to {hash}"))
                    .bind(path)
                    .execute(pool)
                    .await;
                    warning += 1;
                }
            }

            let _ = sqlx::query(
                "INSERT INTO file_integrity_baselines (file_path, sha256_hash, file_size) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (server_id, file_path) DO UPDATE SET sha256_hash = $2, file_size = $3, updated_at = NOW()",
            )
            .bind(path)
            .bind(hash)
            .bind(size)
            .execute(pool)
            .await;
        }
    }

    let total = critical + warning + info;

    let _ = sqlx::query(
        "UPDATE security_scans SET status = 'completed', completed_at = NOW(), \
         findings_count = $1, critical_count = $2, warning_count = $3, info_count = $4 \
         WHERE id = $5",
    )
    .bind(total)
    .bind(critical)
    .bind(warning)
    .bind(info)
    .bind(scan_id)
    .execute(pool)
    .await;

    tracing::info!(
        "Security scan completed: {total} findings ({critical} critical, {warning} warning, {info} info)"
    );

    // Send alerts if critical findings
    if critical > 0 {
        send_scan_alerts(pool, critical, warning, total).await;
    }
}

async fn send_scan_alerts(pool: &PgPool, critical: i32, warning: i32, total: i32) {
    // Get admin emails
    let admins: Vec<(String,)> = sqlx::query_as("SELECT email FROM users WHERE role = 'admin'")
        .fetch_all(pool)
        .await
        .unwrap_or_default();

    let subject = format!("DockPanel Security Alert: {critical} critical findings");
    let body = format!(
        "<h2>Security Scan Results</h2>\
         <p>A scheduled security scan has completed with <strong>{critical} critical</strong> findings.</p>\
         <ul>\
         <li>Critical: {critical}</li>\
         <li>Warning: {warning}</li>\
         <li>Total: {total}</li>\
         </ul>\
         <p>Log in to your DockPanel to review the findings.</p>"
    );

    for (email,) in &admins {
        let _ = crate::services::email::send_email(pool, email, &subject, &body).await;
    }
}
