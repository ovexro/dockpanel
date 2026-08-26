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
                run_scan(&pool, &member, agents.jwt_secret()).await;
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

async fn run_scan(pool: &PgPool, member: &FleetMember, jwt_secret: &str) {
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
        auto_fix_safe_findings(pool, member, findings, jwt_secret).await;
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
        notifications::ssl_renewal_key::DECLINED,
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
        notifications::ssl_renewal_key::FAILED,
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

/// A DNS-01 certificate this loop DECLINED to renew, while the cause is still
/// fixable.
///
/// ⚠ Not `ssl_renewal_alert`. That helper says "SSL renewal failed" and pages at
/// `critical`; neither is true of a refusal the panel made on purpose. This is
/// the same split `ssl_renewal_declined_alert` draws for a foreign issuer, and
/// `ssl-correctness` pins the failure helper's count with a comment saying in so
/// many words that a decline wired back into the failure wording is a defect.
async fn ssl_dns01_declined_alert(
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
        notifications::ssl_renewal_key::DNS01_DECLINED,
        "warning",
        &format!("SSL certificate for {domain} needs a Cloudflare zone"),
        reason,
        12,
    )
    .await;
}

/// A DNS-01 certificate downgraded to a single name on purpose, in its last
/// week. `critical`, because names stopped being covered — but never worded as a
/// failure, because a certificate WAS installed.
async fn ssl_dns01_downgraded_alert(
    pool: &PgPool,
    user_id: uuid::Uuid,
    site_id: uuid::Uuid,
    domain: &str,
    losing: &str,
) {
    notifications::fire_alert_deduped(
        pool,
        user_id,
        None,
        Some(site_id),
        "ssl_renewal_failure",
        notifications::ssl_renewal_key::DNS01_DOWNGRADED,
        "critical",
        &format!("SSL certificate downgraded: {domain}"),
        &format!(
            "The certificate for {domain} covered {losing} and could not be re-ordered over \
             DNS-01 before it expired, so DockPanel issued a single-name certificate for \
             {domain} instead. Any other name it covered is no longer covered."
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
    // The key stored credentials are encrypted under, threaded from `run` so a
    // DNS-01 renewal here can open the Cloudflare token that ISSUED the
    // certificate. This loop has no `AppState` and no session.
    jwt_secret: &str,
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
                    Option<String>,
                )> = sqlx::query_as(
                    "SELECT s.id, s.runtime, s.proxy_port, s.php_version, s.root_path, s.user_id, \
                            s.ssl_profile \
                     FROM sites s WHERE s.domain = $1 AND s.server_id = $2 AND s.ssl_enabled = TRUE",
                )
                .bind(domain)
                .bind(member.id)
                .fetch_optional(pool)
                .await
                .unwrap_or(None);

                let Some((site_id, runtime, proxy_port, php_version, root_path, user_id, ssl_profile)) =
                    site
                else {
                    // The subject came from the agent's FILESYSTEM walk of
                    // /etc/dockpanel/ssl, which is where a Compose stack's ACME
                    // certificate also lands — a stack's domain can never own a
                    // `sites` row (`domain_claim::find_occupant` returns
                    // `Occupant::Stack` and every `INSERT INTO sites` goes
                    // through `ensure_claimable`), so this read cannot ever
                    // match one. Until v2.161.0 a stack fell out of this loop
                    // here with no log, no alert and no renewal: the panel
                    // raised a critical `ssl_expiry` finding naming the domain
                    // every week and then had nothing behind it.
                    renew_stack_certificate(pool, member, domain).await;
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
                // The site's chosen certificate profile travels with the renewal,
                // exactly as it does on the two hand-driven paths. Omitting it does
                // not fail: the CA quietly issues its DEFAULT profile instead, so a
                // site the operator put on `shortlived` or `tlsserver` comes back on
                // the default one while the column still names the retired choice —
                // and the cooldown and margin helpers keep reading that stale column,
                // applying a short-lived certificate's renewal margin to a 90-day one.
                // This is the ONLY automatic renewal on a stock install, so the
                // downgrade is silent, permanent and unattended.
                if let Some(profile) = ssl_profile.as_deref() {
                    agent_body["profile"] = serde_json::json!(profile);
                }

                // WHICH CHALLENGE ISSUED THIS CERTIFICATE decides which door
                // renews it — and this is the door that matters most, because it
                // is the ONLY automatic renewal on a stock install and it is
                // reached with no opt-in at all. `/ssl/provision/{domain}` is
                // the HTTP-01 provisioner, single identifier, writing over
                // `/etc/dockpanel/ssl/{domain}/fullchain.pem`. For a zone-apex
                // wildcard that file is the shared certificate every sibling
                // vhost in the zone is serving.
                let full: Option<crate::models::Site> =
                    sqlx::query_as("SELECT * FROM sites WHERE id = $1")
                        .bind(site_id)
                        .fetch_optional(pool)
                        .await
                        .ok()
                        .flatten();
                let Some(full) = full else { continue };
                let days_remaining = full
                    .ssl_expiry
                    .map(|e| (e - chrono::Utc::now()).num_days());
                let plan = crate::helpers::renewal_plan(pool, &full, days_remaining).await;

                if let crate::helpers::RenewalPlan::Refuse { reason } = &plan {
                    // ⚠ A DIFFERENT sentence from the foreign-issuer stop above.
                    // That line is pinned at exactly one occurrence, and a second
                    // copy of its wording here would let either stop satisfy the
                    // arm written for the other.
                    tracing::warn!("Auto-fix: DNS-01 renewal declined for {domain} — {reason}");
                    ssl_dns01_declined_alert(pool, user_id, site_id, domain, reason).await;
                    continue;
                }

                // ⛔ `post_long` with the shared budget, not `post`. Plain `post`
                // caps at 60s and a wildcard DNS-01 order budgets ~260s in the
                // agent alone.
                let outcome: Result<serde_json::Value, String> = match &plan {
                    crate::helpers::RenewalPlan::Dns01 { subject, wildcard, zone_id } => {
                        crate::routes::ssl::renew_over_dns01(
                            pool,
                            jwt_secret,
                            &agent,
                            subject,
                            *wildcard,
                            *zone_id,
                            user_id,
                            ssl_profile.as_deref(),
                        )
                        .await
                    }
                    _ => agent
                        .post_long(
                            &format!("/ssl/provision/{domain}"),
                            Some(agent_body),
                            crate::routes::ssl::DNS01_ORDER_TIMEOUT_SECS,
                        )
                        .await
                        .map_err(|e| e.to_string()),
                };

                match outcome {
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

                        // Only after success: a failed HTTP-01 attempt on an
                        // unrecorded row must stay unrecorded, because the
                        // likeliest reason it failed is that this site cannot
                        // answer HTTP-01 — the case that made DNS-01 right.
                        crate::routes::ssl::record_renewal_provenance(pool, site_id, domain, &plan)
                            .await;
                        if let crate::helpers::RenewalPlan::LastResortHttp01 { losing } = &plan {
                            ssl_dns01_downgraded_alert(pool, user_id, site_id, domain, losing)
                                .await;
                        }

                        // Same as the auto-healer's success branch: a renewal that
                        // succeeded disproves the alerts saying one had not, and
                        // leaves the downgrade alert above alone.
                        notifications::resolve_ssl_renewal_failure(pool, user_id, site_id, domain)
                            .await;

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
                            // the fix arrive with the PANEL.
                            //
                            // ⛔ RETRACTED at v2.145.0. This comment used to end
                            // "Nothing is lost by skipping: renewal does not
                            // change what the vhost contains, since the
                            // certificate paths it names are stable symlinks."
                            // Both halves are false. Nothing in this tree writes
                            // a symlink under /etc/dockpanel/ssl — both
                            // provisioners `create_dir_all` then `fs::write`
                            // plain files — and a renewal CAN change which path
                            // is correct, because the DNS-01 door writes under
                            // the ZONE apex while the HTTP-01 door writes under
                            // the site's own name. The skip is still right, but
                            // only because a disabled site is not serving:
                            // "nothing is lost" was never the reason.
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

/// Renew the ACME certificate of a Compose STACK whose domain the loop above
/// could not resolve to a site.
///
/// A SIBLING of `auto_fix_safe_findings`, deliberately — not a helper the site
/// path also calls. The site path stays inline and byte-stable: `carry-sweep`
/// §J measures that function's own body for the profile it sends, and an
/// extraction satisfies the arm from a sibling that was never broken.
///
/// Until v2.161.0 nothing in the tree re-ordered a stack's certificate on a
/// schedule. `docker_stacks` carries no `ssl_expiry` and no renewal columns, so
/// every sites-keyed renewer is structurally blind to it; the only thing that
/// re-orders one is an operator-triggered stack create/update/restore. The
/// agent's weekly walk of `/etc/dockpanel/ssl` DID raise the finding, so the
/// operator was warned and then met two dead ends.
///
/// ⚠ No `ssl_expiry` write here, because there is no column to write it to
/// (Tier 2). That is why this path records its outcome in `activity_logs`
/// instead — and why that row is load-bearing, see step 4.
/// The `state_key` a stack renewal failure both FIRES and RESOLVES under.
///
/// ⛔ ONE spelling, computed in ONE place, called by both sides. A fire and a
/// resolve that spell the key separately are a severed pair: the alert fires,
/// the resolve misses, and it pages every thirty minutes for a week about a
/// certificate that is already healthy. That is not hypothetical here — it is
/// the defect `resolve_ssl_renewal_failure` was written for, and this door
/// cannot use that resolver (it takes `site_id: Uuid`, and this alert has none).
///
/// The domain has to be IN the key: `fire_alert_deduped` dedups on
/// `(alert_type, site_id, state_key)` and ignores `server_id`, so with
/// `site_id` NULL every failing stack on every host would otherwise share one
/// twelve-hour bucket and only the first would ever be heard.
///
/// ⚠ `alerts.state_key` is `VARCHAR(100)`. A hostname may be 253 bytes, so the
/// obvious `format!` overflows the column and the INSERT fails — the alert then
/// silently never fires at all, which is worse than the bug it replaced. When
/// the readable form does not fit, the domain is truncated AND a digest of the
/// WHOLE domain is appended: truncation alone would collide two long siblings
/// into the single bucket this key exists to prevent.
fn stack_renewal_state_key(domain: &str) -> String {
    const MAX: usize = 100;
    let readable = format!("stack:{}:{}", notifications::ssl_renewal_key::FAILED, domain);
    if readable.len() <= MAX {
        return readable;
    }
    use sha2::{Digest, Sha256};
    let digest = hex::encode(Sha256::digest(domain.as_bytes()));
    let prefix = format!("stack:{}:", notifications::ssl_renewal_key::FAILED);
    // 16 hex characters of SHA-256 — collision-free for any realistic number of
    // domains on one panel, and it keeps enough of the name to be recognisable.
    //
    // ⚠ Taken by CHARACTER, not by byte. This domain is a directory name read
    // off disk, not a validated field, so a byte slice could land inside a
    // multi-byte sequence and panic — inside the scan loop, taking the whole
    // sweep down. `char_indices` also keeps the result within the byte budget,
    // since every char is at least one byte.
    let budget = MAX - prefix.len() - 17;
    let keep: String = domain.chars().take(budget).collect();
    format!("{prefix}{keep}-{}", &digest[..16])
}

async fn renew_stack_certificate(pool: &PgPool, member: &FleetMember, domain: &str) {
    // 1. Resolve, ON THE HOST THAT RAISED THE FINDING — the same host discipline
    //    the site read above learned the hard way. A domain is unique only per
    //    server, so a name-only lookup on a fleet can hand back another host's
    //    stack and renew through the wrong agent.
    //
    //    ⛔ `lower(domain)`, matching `idx_docker_stacks_domain` and
    //    `domain_claim::find_occupant`. The agent's finding carries whatever
    //    case the certificate directory has.
    let stack: Option<(uuid::Uuid, uuid::Uuid, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, user_id, ssl_email, tls_mode \
         FROM docker_stacks WHERE lower(domain) = lower($1) AND server_id = $2",
    )
    .bind(domain)
    .bind(member.id)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);

    let Some((stack_id, user_id, ssl_email, tls_mode)) = stack else {
        // The silent drop must not survive the fix. A certificate under
        // /etc/dockpanel/ssl belonging to neither a site nor a stack on this
        // host is a real condition (a domain moved, a stack deleted without its
        // certificate) and the operator's only evidence used to be nothing at all.
        // ⛔ `info!`, not `debug!`: nothing shipped runs at debug. `main.rs`
        // defaults the filter to "info" and setup.sh, update.sh and the compose
        // file all pin RUST_LOG=info, so a debug line here is another silent
        // drop wearing a log statement. It fires at most once per expiring
        // certificate per weekly scan.
        tracing::info!(
            "Auto-fix: {domain} is expiring on {} but matches neither a site nor a Compose stack \
             on that host — nothing here can renew it",
            member.name
        );
        return;
    };

    // 2. THE MODE GUARD, and the whole safety of this change.
    //
    //    A `provided` stack serves an operator-supplied certificate from
    //    /etc/dockpanel/ssl-registry/<alias>/. `provision_cert` writes to
    //    {SSL_DIR}/{domain} unconditionally, so ordering for that domain would
    //    at best leave a second unused certificate on disk — and any future
    //    change that let the registry path become the renew target would
    //    replace a paid-for certificate with a Let's Encrypt one, silently,
    //    weekly. `agent/services/ssl.rs` says exactly this in the author's words.
    //
    //    ⛔ Reuse `effective_tls_mode`. The NULL⇒ssl_email rule must have ONE
    //    spelling: a second one in SQL here is the redefinition trap, and the
    //    copy that drifts is the one standing over the operator's certificate.
    let mode = crate::routes::stacks::effective_tls_mode(tls_mode.as_deref(), ssl_email.as_deref());
    if mode != "acme" {
        // `info!` for the same reason as the arm above, and it matters more
        // here: this is the branch that DECLINES to act on a critical finding
        // the operator sees every week. A stack switched to `provided` leaves
        // its old ACME certificate under /etc/dockpanel/ssl, the agent keeps
        // raising `ssl_expiry` for it for ever, and at debug the operator's
        // logs say nothing at all about the domain on their screen.
        tracing::info!(
            "Auto-fix: not renewing {domain} — the stack's TLS mode is '{mode}', not ACME. \
             The expiring file under /etc/dockpanel/ssl is a leftover; the stack serves its \
             registered certificate from the registry."
        );
        return;
    }

    // The agent's `RenewRequest.email` is a required `String`, so a blank
    // address is a 422 rather than a renewal. `effective_tls_mode` can return
    // "acme" with no address when the column literally says so (an operator
    // switch, or a row whose address was cleared), so this is reachable and is
    // NOT the same condition as the mode guard above.
    let Some(contact) = ssl_email.as_deref().map(str::trim).filter(|e| !e.is_empty()) else {
        tracing::warn!(
            "Auto-fix: cannot renew {domain} — the stack is in ACME mode with no ssl_email, \
             and the agent requires a contact address to place an order"
        );
        return;
    };

    // 3. Agent version gate. An agent older than 2.161.0 parses `runtime` as
    //    required and answers 422 to the body below — on every weekly scan.
    //    Read from /health, the route every agent has always carried, and
    //    compared through the same key `require_agent_at_least` uses.
    //
    //    ⛔ A `tracing::warn!` and nothing else. An alert here would page the
    //    operator once a week for ever about a machine that is merely behind.
    let reported = member
        .agent
        .get("/health")
        .await
        .ok()
        .and_then(|v| v.get("version").and_then(|s| s.as_str()).map(str::to_string));
    let key = crate::services::panel_update::semver_key;
    if !(reported.is_some()
        && key(reported.as_deref()) >= key(Some(crate::routes::tls_certificates::STACK_RENEWAL_MIN_AGENT)))
    {
        tracing::warn!(
            "Auto-fix: not renewing the Compose stack certificate for {domain} — {} reports agent \
             {}, and renewing a stack in place needs {} or later. Update that server's agent.",
            member.name,
            reported.as_deref().unwrap_or("no readable version"),
            crate::routes::tls_certificates::STACK_RENEWAL_MIN_AGENT
        );
        return;
    }

    // 3b. THE ISSUER GUARD — what is actually on disk, not what the row claims.
    //
    //     The mode guard above reads a DATABASE COLUMN. The thing at risk is a
    //     FILE, and the column knows nothing about it. The finding that brought
    //     us here came from the agent walking /etc/dockpanel/ssl with
    //     `openssl -checkend`, which never looks at the issuer and never
    //     consults the database — so a stack row saying `acme` proves only what
    //     the panel intended, never what is installed.
    //
    //     A purchased certificate reaches that path by more than one route: the
    //     agent's `upload_cert` door is keyed on DOMAIN and not on a site id and
    //     writes those exact two files; a site that previously owned the name
    //     leaves them behind; an operator installs one by hand. And the
    //     registry migration backfilled EVERY pre-existing stack with a
    //     non-blank `ssl_email` to `acme`, so the mode guard waves all of them
    //     through. Ordering then has `provision_cert` overwrite fullchain.pem
    //     and privkey.pem in place, unattended, weekly, with no alert.
    //
    //     Every other renewal door in the tree asks this question first — the
    //     site arm of THIS function, `auto_healer`, `routes/ssl` and
    //     `routes/mail`. This one was the only door that did not, and the
    //     contract it was built from did not ask for it.
    //
    //     `None` means "not proven foreign" — an unreachable agent, an
    //     unreadable certificate — and MUST still renew, for the same reason the
    //     site arm gives: refusing on doubt lets a genuine certificate lapse,
    //     which is the failure this loop exists to prevent.
    if let Some(issuer) = crate::helpers::foreign_cert_issuer(&member.agent, domain).await {
        tracing::info!(
            "Auto-fix: NOT renewing the Compose stack certificate for {domain} — the installed \
             certificate was issued by {issuer}, not by DockPanel. Renewing would replace it."
        );
        return;
    }

    // 4. THE COOLDOWN, counting the rows step 6 writes.
    //
    //    ⚠ Its own action string, deliberately. Sharing `auto_heal.renew_ssl`
    //    would let either gate satisfy the other's arm and let one loop's
    //    attempt mute the other's.
    //    ⛔ SERVER-SCOPED, for the same reason the lookup in step 1 is. A domain
    //       is unique only per server, and step 6 stamps every row it writes with
    //       the host that raised the finding. Counting name-only made two hosts
    //       carrying the same domain share ONE six-hour budget: host A's renewal
    //       muted host B's, so B's certificate could sit unrenewed for as long as
    //       A kept succeeding — the exact fleet hazard this file's host discipline
    //       exists to prevent, in the one query that had not learned it.
    let recent: Option<(i64,)> = sqlx::query_as(
        "SELECT COUNT(*) FROM activity_logs \
         WHERE action = 'auto_fix.renew_stack_ssl' \
         AND target_name = $1 \
         AND server_id = $2 \
         AND created_at > NOW() - INTERVAL '6 hours'",
    )
    .bind(domain)
    .bind(member.id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    if recent.map(|r| r.0).unwrap_or(0) > 0 {
        return;
    }

    tracing::info!("Auto-fix: renewing the Compose stack certificate for {domain}");

    // 5. Act. NO `runtime` key: that absence is the contract with the agent's
    //    in-place branch, which reloads nginx rather than re-rendering a vhost
    //    the panel cannot describe (it does not know the stack's published
    //    port — the agent derives that from the compose YAML, and a re-render
    //    would emit a proxy vhost with no upstream). The certificate paths do
    //    not move: `provision_cert` overwrites fullchain.pem and privkey.pem in
    //    place, and the stack's acme vhost already names exactly those paths.
    //
    //    No `profile` either — `docker_stacks` has no `ssl_profile` column.
    //
    //    ⛔ `post_long` with the shared budget, not `post`: a plain `post` caps
    //    at 60s and an ACME order budgets far more in the agent alone.
    let result = member
        .agent
        .post_long(
            &format!("/ssl/{domain}/renew"),
            Some(serde_json::json!({ "email": contact })),
            crate::routes::ssl::DNS01_ORDER_TIMEOUT_SECS,
        )
        .await
        .map_err(|e| e.to_string());

    let success = result.is_ok();

    // 6. Record — AND THIS ROW IS THE COOLDOWN step 4 counts.
    //
    //    ⚠ `auto_healer.rs` is the cautionary tale, in this exact shape: it
    //    passed `uuid::Uuid::nil()`, `fk_activity_logs_user` rejected the
    //    insert, the logger swallowed the error into a warn, `COUNT(*)` was
    //    therefore permanently 0, and a certificate that could not be renewed
    //    was re-ordered from the CA every 120 seconds, for ever. Named against
    //    the STACK'S OWN `user_id` (a real `users` row) and stamped with the
    //    host that raised the finding.
    //
    //    Written on BOTH outcomes. A failure that logged nothing would leave
    //    the gate above counting zero and re-order weekly against a CA that
    //    just refused — the same hammering with a longer period.
    let details = match &result {
        Ok(v) => v.to_string(),
        Err(e) => e.clone(),
    };
    crate::services::activity::log_activity_on_server(
        pool,
        user_id,
        "security-scanner",
        "auto_fix.renew_stack_ssl",
        Some("stack"),
        Some(domain),
        Some(&format!("stack_id={stack_id}, success={success}, result={details}")),
        None,
        Some(member.id),
    )
    .await;

    match result {
        Ok(ref v) => {
            tracing::info!("Auto-fix: Compose stack certificate renewed for {domain}");

            // ⛔ THE PARSE IS MANDATORY — the wire value and the column are not the
            //    same kind of thing. The agent prints the `time` crate's Display
            //    (`2026-10-23 09:41:07.0 +00:00:00`), a STRING; `ssl_expiry` is
            //    TIMESTAMPTZ. Binding the string straight in is rejected by
            //    Postgres, and `parse_agent_cert_expiry` exists for exactly this
            //    shape — its own unit test uses that literal as the proof.
            //
            // ⛔ `stack_id` is the id resolved in step 1. Re-reading the row to
            //    fetch it would add a second `FROM docker_stacks WHERE
            //    lower(domain) = lower($1) AND server_id = $2` and a second
            //    `.bind(domain).bind(member.id)` — turning the two arms that
            //    protect this door's host discipline red for no gain.
            //
            //    A failed UPDATE is NOT a failed renewal: the certificate is
            //    already on disk. Recording nothing is a stale row, not a lost
            //    certificate, and the next scan rewrites it.
            match v
                .get("expiry")
                .and_then(|x| x.as_str())
                .and_then(crate::helpers::parse_agent_cert_expiry)
            {
                Some(expiry) => {
                    if let Err(e) = sqlx::query(
                        "UPDATE docker_stacks SET ssl_expiry = $1 WHERE id = $2",
                    )
                    .bind(expiry)
                    .bind(stack_id)
                    .execute(pool)
                    .await
                    {
                        tracing::warn!(
                            "Auto-fix: renewed {domain} but could not record its expiry: {e}"
                        );
                    }
                }
                // Wire-format drift, and `info!` because nothing shipped runs at
                // debug. The renewal succeeded; only the bookkeeping did not.
                None => tracing::info!(
                    "Auto-fix: renewed {domain} but the agent's expiry was unreadable: {:?}",
                    v.get("expiry")
                ),
            }
            crate::services::system_log::log_event(
                pool,
                "info",
                "security_scanner",
                &format!("Auto-renewed the Compose stack SSL certificate for {domain}"),
                None,
            )
            .await;

            // ⛔ A SUCCESS MUST CLEAR THE FAILURE IT FOLLOWS. Without this the
            // alert raised by a previous week's transient failure stays
            // `firing` for ever: `check_escalations` re-pages every thirty
            // minutes for seven days about a certificate that is already
            // healthy, the dashboard's firing count never returns to zero, and
            // neither retention sweep collects it (both delete only
            // `status = 'resolved'`). Nothing else can clear it — the sibling
            // resolver iterates a fixed key list and takes a non-optional
            // `site_id`, and this alert has neither.
            //
            // Fired with `server_id = Some(member.id)`, so `resolve_alert`'s
            // first arm matches on exactly the same four columns. The key comes
            // from the shared function, so the two can never drift apart.
            notifications::resolve_alert(
                pool,
                user_id,
                Some(member.id),
                None,
                "ssl_renewal_failure",
                &stack_renewal_state_key(domain),
                &format!("SSL renewal recovered: {domain}"),
                &format!(
                    "DockPanel renewed the certificate for the Compose stack on {domain}. \
                     The earlier renewal failure is resolved."
                ),
            )
            .await;
        }
        Err(e) => {
            tracing::warn!("Auto-fix: Compose stack SSL renewal failed for {domain}: {e}");
            // 7. `site_id = None` — there is no `sites` row and inventing one
            //    would attach this alert to somebody else's site.
            //
            //    ⚠ `fire_alert_deduped` dedups on (alert_type, site_id,
            //    state_key) and does NOT consider `server_id`. Every existing
            //    `ssl_renewal_failure` caller passes a real `site_id`, so the
            //    key alone separates them; with `site_id` NULL a bare
            //    `ssl_renewal_key::FAILED` would put EVERY failing stack on
            //    EVERY host into one twelve-hour bucket and the second domain's
            //    critical alert would never reach anybody. The subject is the
            //    domain, so the key names it — see `stack_renewal_state_key`,
            //    which is also what the success arm resolves on.
            notifications::fire_alert_deduped(
                pool,
                user_id,
                Some(member.id),
                None,
                "ssl_renewal_failure",
                &stack_renewal_state_key(domain),
                "critical",
                &format!("SSL renewal failed: {domain}"),
                &format!(
                    "The certificate for the Compose stack on {domain} is expiring and DockPanel \
                     could not renew it automatically: {e}. The stack will stop loading over \
                     HTTPS when the certificate expires. Redeploying the stack reissues it."
                ),
                12,
            )
            .await;
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
