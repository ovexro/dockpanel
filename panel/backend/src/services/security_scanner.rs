use sqlx::PgPool;
use std::time::Duration;

use crate::services::agent::AgentClient;
use crate::services::notifications;

/// Background task: runs weekly security scans automatically.
pub async fn run(pool: PgPool, agent: AgentClient, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Security scanner background task started (weekly)");

    // Initial delay: 5 minutes after startup (respects shutdown)
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(300)) => {}
        _ = shutdown_rx.recv() => {
            tracing::info!("Security scanner shutting down gracefully (during initial delay)");
            return;
        }
    }

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

        // Check every 6 hours if a weekly scan is due (respects shutdown)
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(6 * 3600)) => {}
            _ = shutdown_rx.recv() => {
                tracing::info!("Security scanner shutting down gracefully");
                return;
            }
        }
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
            crate::services::system_log::log_event(
                pool,
                "error",
                "security_scanner",
                "Scheduled security scan failed",
                Some(&e.to_string()),
            ).await;
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

    // Auto-fix safe findings (non-destructive only)
    if let Some(findings) = findings {
        auto_fix_safe_findings(pool, agent, findings).await;
    }

    // Keep only last 90 days of scans
    let _ = sqlx::query("DELETE FROM security_findings WHERE scan_id IN (SELECT id FROM security_scans WHERE created_at < NOW() - INTERVAL '90 days')")
        .execute(pool).await;
    let _ = sqlx::query("DELETE FROM security_scans WHERE created_at < NOW() - INTERVAL '90 days'")
        .execute(pool).await;

    // Auto-resolve prior firing security alerts so the new scan's result is
    // the single source of truth — avoids the "every 2–5 min escalation on
    // three stale alerts" pileup the user saw on 2026-04-15.
    let _ = sqlx::query(
        "UPDATE alerts SET status = 'resolved', resolved_at = NOW() \
         WHERE alert_type = 'security' AND status IN ('firing', 'acknowledged')",
    )
    .execute(pool)
    .await;

    // Send alerts if critical or warning findings
    if critical > 0 || warning > 0 {
        send_scan_alerts(pool, critical, warning, total).await;
    } else {
        // Clean scan — notify panel
        notifications::notify_panel(pool, None, "Security scan: all clear", "No vulnerabilities or issues detected", "info", "security", Some("/security")).await;
    }
}

/// Auto-fix safe findings after a scan completes.
/// Only fixes things that are SAFE to fix automatically (SSL renewal).
/// Never auto-fixes malware, open ports, or config changes that could break things.
async fn auto_fix_safe_findings(
    pool: &PgPool,
    agent: &AgentClient,
    findings: &[serde_json::Value],
) {
    for f in findings {
        let check_type = f["check_type"].as_str().unwrap_or("");
        match check_type {
            // Auto-renew expiring SSL certs
            "ssl_expiry" => {
                // Extract domain from the title: "SSL certificate expiring: example.com"
                let title = f["title"].as_str().unwrap_or("");
                let domain = title.strip_prefix("SSL certificate expiring: ").unwrap_or("");
                if domain.is_empty() {
                    continue;
                }

                // Look up site details from DB (same pattern as auto_healer)
                let site: Option<(String, Option<i32>, Option<String>, Option<String>, uuid::Uuid)> =
                    sqlx::query_as(
                        "SELECT s.runtime, s.proxy_port, s.php_version, s.root_path, s.user_id \
                         FROM sites s WHERE s.domain = $1 AND s.ssl_enabled = TRUE",
                    )
                    .bind(domain)
                    .fetch_optional(pool)
                    .await
                    .unwrap_or(None);

                let Some((runtime, proxy_port, php_version, root_path, user_id)) = site else {
                    continue;
                };

                // Look up owner email for ACME registration
                let email: Option<String> = sqlx::query_scalar(
                    "SELECT email FROM users WHERE id = $1",
                )
                .bind(user_id)
                .fetch_optional(pool)
                .await
                .unwrap_or(None);

                let Some(email) = email else {
                    tracing::warn!("Auto-fix: cannot renew SSL for {domain} — owner email not found");
                    continue;
                };

                tracing::info!("Auto-fix: renewing expiring SSL certificate for {domain}");

                let mut agent_body = serde_json::json!({
                    "email": email,
                    "runtime": runtime,
                });
                if let Some(port) = proxy_port {
                    agent_body["proxy_port"] = serde_json::json!(port);
                }
                if let Some(php) = &php_version {
                    agent_body["php_socket"] =
                        serde_json::json!(format!("/run/php/php{php}-fpm.sock"));
                }
                if let Some(root) = &root_path {
                    agent_body["root"] = serde_json::json!(root);
                }

                match agent
                    .post(
                        &format!("/ssl/provision/{domain}"),
                        Some(agent_body),
                    )
                    .await
                {
                    Ok(_) => {
                        tracing::info!("Auto-fix: SSL renewed successfully for {domain}");
                        crate::services::system_log::log_event(
                            pool,
                            "info",
                            "security_scanner",
                            &format!("Auto-renewed SSL certificate for {domain}"),
                            None,
                        )
                        .await;
                    }
                    Err(e) => {
                        tracing::warn!("Auto-fix: SSL renewal failed for {domain}: {e}");
                    }
                }
            }
            // Security headers — log as recommendation only
            "security_headers" => {
                tracing::info!(
                    "Auto-fix: security headers — logged as recommendation for {}",
                    f["title"].as_str().unwrap_or("unknown")
                );
                // Headers are already in nginx templates — this finding means custom config.
                // Don't auto-fix, just log.
            }
            // Don't auto-fix: malware, open_port, container_vuln, file_integrity
            _ => {}
        }
    }
}

async fn send_scan_alerts(pool: &PgPool, critical: i32, warning: i32, total: i32) {
    // Get admin users to create alerts for
    let admins: Vec<(uuid::Uuid, String)> =
        sqlx::query_as("SELECT id, email FROM users WHERE role = 'admin'")
            .fetch_all(pool)
            .await
            .unwrap_or_default();

    let severity = if critical > 0 { "critical" } else { "warning" };
    let title = format!(
        "Security scan: {} critical, {} warning findings",
        critical, warning
    );
    let message = format!(
        "A scheduled security scan completed with {} total findings ({} critical, {} warning). \
         Review the scan results in the Security section.",
        total, critical, warning
    );

    // Create an alert for each admin user via the alerts system
    for (user_id, _email) in &admins {
        notifications::fire_alert(
            pool,
            *user_id,
            None,
            None,
            "security",
            severity,
            &title,
            &message,
        )
        .await;
    }
}
