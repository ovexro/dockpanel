use sqlx::PgPool;
use std::time::Duration;

use crate::services::agent::{AgentRegistry, FleetMember};
use crate::services::notifications;

/// Background task: runs weekly security scans automatically, on every server.
///
/// This scanner's subject is a MACHINE, not a row — it asks an agent what it
/// found — so there was no per-row `server_id` to thread and it needs the
/// fleet loop instead. Until v2.58.0 it asked the panel's own agent once a week
/// and recorded the answer with a NULL host, so a fleet's members were never
/// scanned at all: not mislabelled, simply never looked at.
pub async fn run(pool: PgPool, agents: AgentRegistry, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
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
        for member in agents.online_fleet().await {
            // Is THIS server due? The cadence gate used to be fleet-wide, which
            // on a fleet meant the first host to be scanned satisfied it for
            // every other host — one machine scanned weekly and the rest never.
            //
            // It also counted rows of ANY status, so a single failed or
            // interrupted scan bought a whole week of no security scanning at
            // all. Only a COMPLETED scan proves the machine was looked at.
            let recent: Option<(i64,)> = sqlx::query_as(
                "SELECT COUNT(*) FROM security_scans \
                 WHERE server_id = $1 AND status = 'completed' \
                   AND created_at > NOW() - INTERVAL '7 days'",
            )
            .bind(member.id)
            .fetch_optional(&pool)
            .await
            .unwrap_or(None);

            let needs_scan = recent.map(|(c,)| c == 0).unwrap_or(true);

            if needs_scan {
                tracing::info!("Running scheduled weekly security scan on {}", member.name);
                run_scan(&pool, &member).await;
            }
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

async fn run_scan(pool: &PgPool, member: &FleetMember) {
    let agent = &member.agent;

    // Create scan record, naming the host it is a scan OF.
    let scan_id: uuid::Uuid = match sqlx::query_scalar(
        "INSERT INTO security_scans (server_id, scan_type, status) VALUES ($1, 'full', 'running') RETURNING id",
    )
    .bind(member.id)
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

            // One row per (server, path) now that the upsert below actually
            // upserts, so this is a keyed lookup rather than "whichever of the
            // eighteen duplicates the heap returned first".
            let existing: Option<(String,)> = sqlx::query_as(
                "SELECT sha256_hash FROM file_integrity_baselines \
                 WHERE server_id = $1 AND file_path = $2",
            )
            .bind(member.id)
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

            // Binding the host is what ARMS this upsert. The arbiter named a
            // column the INSERT never supplied, and a NULL never conflict-matches,
            // so DO UPDATE had never executed and the baseline never advanced
            // past the first scan.
            let _ = sqlx::query(
                "INSERT INTO file_integrity_baselines (server_id, file_path, sha256_hash, file_size) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (server_id, file_path) DO UPDATE SET sha256_hash = $3, file_size = $4, updated_at = NOW()",
            )
            .bind(member.id)
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
        auto_fix_safe_findings(pool, member, findings).await;
    }

    // Keep only last 90 days of scans
    let _ = sqlx::query("DELETE FROM security_findings WHERE scan_id IN (SELECT id FROM security_scans WHERE created_at < NOW() - INTERVAL '90 days')")
        .execute(pool).await;
    let _ = sqlx::query("DELETE FROM security_scans WHERE created_at < NOW() - INTERVAL '90 days'")
        .execute(pool).await;

    // Auto-resolve prior firing security alerts so the new scan's result is
    // the single source of truth — avoids the "every 2–5 min escalation on
    // three stale alerts" pileup the user saw on 2026-04-15.
    // Scoped to the scanned host: a clean scan on one machine must not silently
    // close another machine's outstanding security alerts.
    let _ = sqlx::query(
        "UPDATE alerts SET status = 'resolved', resolved_at = NOW() \
         WHERE alert_type = 'security' AND server_id = $1 AND status IN ('firing', 'acknowledged')",
    )
    .bind(member.id)
    .execute(pool)
    .await;

    // Send alerts if critical or warning findings
    if critical > 0 || warning > 0 {
        send_scan_alerts(pool, member, critical, warning, total).await;
    } else {
        // Clean scan — notify panel
        notifications::notify_panel(
            pool,
            None,
            "Security scan: all clear",
            &format!("No vulnerabilities or issues detected on {}", member.name),
            "info",
            "security",
            Some("/security"),
        )
        .await;
    }
}

/// Raise an alert when a certificate this scanner found to be expiring cannot be
/// renewed. Deduped per site so the six-hourly scan cannot stack up duplicates
/// alongside the auto-healer's own alert for the same certificate.
/// A renewal DockPanel deliberately did not perform, because the certificate is
/// not one it issued.
///
/// Deliberately NOT `ssl_renewal_alert`. That helper's words are
/// "SSL renewal failed", "could not renew it automatically" and severity
/// `critical` — three statements that are all false here. Nothing failed and
/// nothing was prevented: the panel declined, correctly, to overwrite somebody
/// else's certificate. Sending that operator a critical page about a failure
/// would be the same defect this release exists to remove, one layer further
/// out — and it would train them to ignore the alert that means their site is
/// about to go dark.
///
/// The `alert_type` is unchanged so routing, dedupe and every notification lane
/// behave exactly as before; only the wording and the severity differ.
async fn ssl_renewal_declined_alert(
    pool: &PgPool,
    user_id: uuid::Uuid,
    site_id: uuid::Uuid,
    domain: &str,
    issuer: &str,
) {
    notifications::fire_alert_deduped(
        pool,
        user_id,
        None,
        Some(site_id),
        "ssl_renewal_failure",
        "",
        "warning",
        &format!("SSL certificate for {domain} needs renewing by you"),
        &format!(
            "The certificate for {domain} is expiring, and DockPanel did not renew it \
             because it did not issue it: it was issued by {issuer}. Renewing here would \
             have replaced your certificate with a Let's Encrypt one. Renew it wherever it \
             was issued and upload the replacement under the site's SSL tab."
        ),
        12,
    )
    .await;
}

async fn ssl_renewal_alert(
    pool: &PgPool,
    user_id: uuid::Uuid,
    site_id: uuid::Uuid,
    domain: &str,
    reason: &str,
) {
    notifications::fire_alert_deduped(
        pool,
        user_id,
        None,
        Some(site_id),
        "ssl_renewal_failure",
        "",
        "critical",
        &format!("SSL renewal failed: {domain}"),
        &format!(
            "The certificate for {domain} is expiring and DockPanel could not renew \
             it automatically: {reason}. The site will stop loading when the \
             certificate expires."
        ),
        12,
    )
    .await;
}

/// Auto-fix safe findings after a scan completes.
/// Only fixes things that are SAFE to fix automatically (SSL renewal).
/// Never auto-fixes malware, open ports, or config changes that could break things.
async fn auto_fix_safe_findings(
    pool: &PgPool,
    member: &FleetMember,
    findings: &[serde_json::Value],
) {
    let agent = &member.agent;
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

                // Look up site details from DB, ON THE HOST THAT RAISED THE FINDING.
                //
                // ⚠ This read used to be `WHERE s.domain = $1` alone, and the
                // hazard is spelled out 130 lines below on the vhost read: a
                // domain is unique only per server (`idx_sites_domain_server`,
                // migration `20260319000000_multi_server.sql:84`), so a lookup
                // on the name alone can hand back another host's row. That
                // comment was written about the SECOND read while the FIRST one
                // — the read that produces the very `site_id` it then trusts —
                // still had the defect. `fetch_optional` takes whichever row the
                // planner returns first, so on a fleet carrying the same domain
                // twice this renewed with the other host's runtime/root/php
                // config, wrote the new expiry onto the other host's row (so the
                // certificate that was actually renewed kept counting down), and
                // pushed that host's full vhost through THIS host's agent.
                // Unattended, on the scan loop's own schedule.
                let site: Option<(
                    uuid::Uuid,
                    String,
                    Option<i32>,
                    Option<String>,
                    Option<String>,
                    uuid::Uuid,
                )> = sqlx::query_as(
                    "SELECT s.id, s.runtime, s.proxy_port, s.php_version, s.root_path, s.user_id \
                     FROM sites s WHERE s.domain = $1 AND s.server_id = $2 AND s.ssl_enabled = TRUE",
                )
                .bind(domain)
                .bind(member.id)
                .fetch_optional(pool)
                .await
                .unwrap_or(None);

                let Some((site_id, runtime, proxy_port, php_version, root_path, user_id)) = site
                else {
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

                let Some(owner_email) = email else {
                    tracing::warn!("Auto-fix: cannot renew SSL for {domain} — owner email not found");
                    ssl_renewal_alert(
                        pool,
                        user_id,
                        site_id,
                        domain,
                        "the site's owner account has no email address on file",
                    )
                    .await;
                    continue;
                };

                // Resolve through the SAME path issuance uses. Without this the
                // panel-wide `acme_contact_email` rescue applies only when a human
                // clicks, so a box whose owner address cannot be an ACME contact
                // (reserved TLD, typo) issues fine and then silently fails to renew
                // ~60 days later.
                let email = match crate::routes::ssl::resolve_acme_contact(pool, &owner_email).await {
                    Ok(addr) => addr,
                    Err(reason) => {
                        tracing::warn!("Auto-fix: cannot renew SSL for {domain} — {reason}");
                        ssl_renewal_alert(pool, user_id, site_id, domain, &reason).await;
                        continue;
                    }
                };

                // ⛔ DO NOT REPLACE A CERTIFICATE THIS PRODUCT DID NOT ISSUE.
                //
                // This is the ONLY automatic renewal on a stock install and it is
                // reached with no opt-in at all, so what it does unattended is what
                // the product does by default. What it did was a full ACME order
                // writing the same `fullchain.pem` an uploaded certificate occupies:
                // the finding it acts on comes from the AGENT walking
                // `/etc/dockpanel/ssl` with `openssl -checkend`, which never looks at
                // the issuer and never consults the database — so a commercial
                // wildcard, a Cloudflare Origin CA certificate or a corporate PKI
                // certificate uploaded through the panel's own "upload certificate"
                // control was silently destroyed roughly a month before it expired,
                // every week, on every install. The operator's evidence was a working
                // site that had quietly stopped presenting the certificate they paid
                // for.
                //
                // `None` means "not proven foreign" — an unreachable agent, an
                // unreadable certificate — and MUST still renew. Refusing on doubt
                // would let a genuine Let's Encrypt certificate lapse, which is the
                // failure this loop exists to prevent.
                if let Some(issuer) =
                    crate::helpers::foreign_cert_issuer(agent, domain).await
                {
                    tracing::info!(
                        "Auto-fix: NOT renewing {domain} — the installed certificate was \
                         issued by {issuer}, not by DockPanel. Renewing would replace it."
                    );
                    ssl_renewal_declined_alert(pool, user_id, site_id, domain, &issuer).await;
                    continue;
                }

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
                        serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
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
                    Ok(result) => {
                        tracing::info!("Auto-fix: SSL renewed successfully for {domain}");

                        // Record the certificate we just installed. The agent
                        // returns `expiry` and every other renewal path stores
                        // it; this one dropped it, so the panel's view stayed
                        // frozen at the value written when the certificate was
                        // first issued. The dashboard countdown ran to zero and
                        // `check_ssl_expiry` walked the whole warning ladder
                        // down to the EXPIRED sentinel on a certificate that had
                        // renewed perfectly — and could never recover, because
                        // `ssl_decision` resolves only when `days_left` RISES,
                        // which cannot happen while nothing rewrites this
                        // column.
                        //
                        // This is the ONLY automatic renewal on a stock install
                        // (`auto_heal_enabled` is seeded false) and it runs for
                        // every host in `online_fleet()`, the panel's own
                        // included — so the stale value was not a fleet-only
                        // condition.
                        //
                        // All three columns move together, as in
                        // `auto_healer::auto_renew_ssl` and `ssl::renew`: a
                        // surviving `ssl_renewal_at` is an ARI window computed
                        // for the certificate this one just replaced.
                        match result
                            .get("expiry")
                            .and_then(|v| v.as_str())
                            .and_then(crate::helpers::parse_agent_cert_expiry)
                        {
                            Some(expiry) => {
                                let _ = sqlx::query(
                                    "UPDATE sites SET ssl_expiry = $1, ssl_renewal_at = NULL, \
                                     ssl_renewal_checked_at = NULL, updated_at = NOW() WHERE id = $2",
                                )
                                .bind(expiry)
                                .bind(site_id)
                                .execute(pool)
                                .await;
                            }
                            None => {
                                tracing::warn!(
                                    "Auto-fix: renewed {domain} but could not read the new expiry \
                                     from the agent (raw: {:?}) — the countdown and the expiry \
                                     alert will keep describing the retired certificate",
                                    result.get("expiry")
                                );
                            }
                        }

                        // Preserve the site's full config (WAF/CSP/Permissions-
                        // Policy/rate-limit/custom_nginx/bot-protection) — the
                        // agent's provision only renders a subset. Best-effort.
                        //
                        // Keyed on the id already resolved above, not on the
                        // domain: `domain` is unique only per server
                        // (`idx_sites_domain_server`), so a lookup on it could
                        // hand back another host's row and push that host's
                        // vhost through THIS host's agent.
                        if let Ok(site) = sqlx::query_as::<_, crate::models::Site>("SELECT * FROM sites WHERE id = $1")
                            .bind(site_id)
                            .fetch_one(pool)
                            .await
                        {
                            // A site the operator disabled must not be put back
                            // into service by a renewal nobody watched. The
                            // rebuild used to go out unconditionally, and the
                            // agent wrote it straight over the maintenance
                            // response — so a deliberately offline site came
                            // back on the internet on this loop's own schedule,
                            // while the panel went on showing it as disabled.
                            //
                            // Agents from v2.74.0 park the body instead of
                            // serving it, but this loop runs against every host
                            // in the fleet and an agent is only updated when
                            // somebody updates it. Declining here is what makes
                            // the fix arrive with the PANEL. Nothing is lost by
                            // skipping: renewal does not change what the vhost
                            // contains, since the certificate paths it names are
                            // stable symlinks.
                            if !site.enabled {
                                tracing::info!(
                                    "Auto-fix: renewed SSL for {} but skipped the vhost rebuild — the site is disabled",
                                    site.domain
                                );
                            } else if let Err(e) = agent
                                .put(
                                    &format!("/nginx/sites/{}", site.domain),
                                    crate::routes::sites::build_nginx_body(&site),
                                )
                                .await
                            {
                                tracing::warn!("Auto-fix: full vhost rebuild after SSL renewal failed for {}: {e}", site.domain);
                            }
                        }

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
                        // The auto-healer alerts when its own renewal fails; this
                        // path used to end at the log line, so whether a failing
                        // renewal was visible depended on which loop reached the
                        // certificate first.
                        ssl_renewal_alert(pool, user_id, site_id, domain, &e.to_string()).await;
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

async fn send_scan_alerts(pool: &PgPool, member: &FleetMember, critical: i32, warning: i32, total: i32) {
    // Get admin users to create alerts for
    let admins: Vec<(uuid::Uuid, String)> =
        sqlx::query_as("SELECT id, email FROM users WHERE role = 'admin'")
            .fetch_all(pool)
            .await
            .unwrap_or_default();

    let severity = if critical > 0 { "critical" } else { "warning" };
    let title = format!(
        "Security scan on {}: {} critical, {} warning findings",
        member.name, critical, warning
    );
    let message = format!(
        "A scheduled security scan of {} completed with {} total findings ({} critical, {} warning). \
         Review the scan results in the Security section.",
        member.name, total, critical, warning
    );

    // Create an alert for each admin user via the alerts system, naming the
    // scanned server — without it the operator gets identical alerts from every
    // machine in the fleet and no way to tell which one is on fire.
    for (user_id, _email) in &admins {
        notifications::fire_alert(
            pool,
            *user_id,
            Some(member.id),
            None,
            "security",
            "",
            severity,
            &title,
            &message,
        )
        .await;
    }
}
