use crate::safe_cmd::safe_command;
use sqlx::PgPool;
use std::time::{Duration, Instant};

#[derive(sqlx::FromRow, Clone)]
struct MonitorRow {
    id: uuid::Uuid,
    user_id: uuid::Uuid,
    site_id: Option<uuid::Uuid>,
    url: String,
    name: String,
    status: String,
    alert_email: bool,
    alert_slack_url: Option<String>,
    alert_discord_url: Option<String>,
    monitor_type: String,
    port: Option<i32>,
    keyword: Option<String>,
    keyword_must_contain: bool,
    check_interval: i32,
    last_checked_at: Option<chrono::DateTime<chrono::Utc>>,
    custom_headers: Option<serde_json::Value>,
}

/// Background task: checks all enabled monitors periodically.
pub async fn run(pool: PgPool, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Uptime monitor started");
    crate::services::status_notices::start_worker(pool.clone());
    let client = http_check_client_builder().build().unwrap();

    let mut interval = tokio::time::interval(Duration::from_secs(60));
    let mut tick_count: u64 = 0;

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown_rx.recv() => {
                tracing::info!("Uptime monitor shutting down gracefully");
                return;
            }
        }

        tick_count += 1;

        // Get monitors due for checking (HTTP/TCP/ping) + all heartbeat monitors (checked separately)
        let monitors: Vec<MonitorRow> = match sqlx::query_as(
            "SELECT id, user_id, site_id, url, name, status, alert_email, alert_slack_url, alert_discord_url, \
             monitor_type, port, keyword, keyword_must_contain, check_interval, last_checked_at, custom_headers \
             FROM monitors WHERE enabled = TRUE AND \
             (monitor_type = 'heartbeat' OR last_checked_at IS NULL OR last_checked_at < NOW() - (check_interval || ' seconds')::interval)",
        )
        .fetch_all(&pool)
        .await
        {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Uptime monitor query error: {e}");
                continue;
            }
        };

        // Batch-load users in maintenance windows (avoid N+1 query per monitor)
        let maintenance_users: std::collections::HashSet<uuid::Uuid> = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT DISTINCT user_id FROM maintenance_windows WHERE starts_at <= NOW() AND ends_at >= NOW()"
        ).fetch_all(&pool).await.unwrap_or_default().into_iter().collect();

        // Process monitors concurrently (max 10 at a time)
        let mut set = tokio::task::JoinSet::new();
        for monitor in monitors {
            // Skip monitors for users in maintenance windows
            if maintenance_users.contains(&monitor.user_id) {
                continue;
            }

            let c = client.clone();
            let p = pool.clone();
            set.spawn(async move {
                check_monitor(&monitor, &c, &p).await;
            });
            // Cap concurrency at 10 — wait for one to finish before spawning more
            if set.len() >= 10 {
                let _ = set.join_next().await;
            }
        }
        // Drain remaining tasks
        while let Some(_) = set.join_next().await {}

        // Purge old data only every hour (every 60th tick at 60s interval)
        if tick_count % 60 == 0 {
            // Purge old check records (keep last 24h)
            if let Err(e) = sqlx::query(
                "DELETE FROM monitor_checks WHERE checked_at < NOW() - INTERVAL '24 hours'",
            )
            .execute(&pool)
            .await {
                tracing::error!("Failed to purge old monitor checks: {e}");
            }

            // Purge old performance metrics (keep last 7 days)
            if let Err(e) = sqlx::query(
                "DELETE FROM metrics WHERE recorded_at < NOW() - INTERVAL '7 days'",
            )
            .execute(&pool)
            .await {
                tracing::error!("Failed to purge old metrics: {e}");
            }
        }
    }
}

/// Check a single monitor: HTTP/TCP/ping request, record result, handle status transitions.
async fn check_monitor(monitor: &MonitorRow, client: &reqwest::Client, pool: &PgPool) {
    // Heartbeat monitors are passive — check if we missed a beat
    if monitor.monitor_type == "heartbeat" {
        check_heartbeat(monitor, pool).await;
        return;
    }

    let (status_code, error, new_status, response_time) = match monitor.monitor_type.as_str() {
        "tcp" => check_tcp(monitor).await,
        "ping" => check_ping(monitor).await,
        _ => check_http(monitor, client).await,
    };

    // Insert check record
    if let Err(e) = sqlx::query(
        "INSERT INTO monitor_checks (monitor_id, status_code, response_time, error) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(monitor.id)
    .bind(status_code)
    .bind(response_time)
    .bind(&error)
    .execute(pool)
    .await {
        tracing::error!("Failed to insert monitor check for {}: {e}", monitor.name);
    }

    // Update monitor status
    if let Err(e) = sqlx::query(
        "UPDATE monitors SET status = $1, last_checked_at = NOW(), \
         last_response_time = $2, last_status_code = $3 WHERE id = $4",
    )
    .bind(new_status)
    .bind(response_time)
    .bind(status_code)
    .bind(monitor.id)
    .execute(pool)
    .await {
        tracing::error!("Failed to update monitor status for {}: {e}", monitor.name);
    }

    // GAP 29: Response time degradation alerting
    // If the site is technically up but very slow (>5s), fire a warning alert
    //
    // This statement could not succeed in any database, and its error was
    // discarded, so from the day it was written until it was rewritten no
    // slow-response alert has ever existed: no Dashboard count, no row on the
    // Alerts page, no Prometheus sample. Three independent faults — it named a
    // column `subject` that the table does not have (the column is `title`),
    // omitted `title` which is NOT NULL with no default, and bound a `$2` the
    // SQL never referenced. It also attributed the alert to whichever server was
    // created first on the panel rather than to the monitor's own site.
    //
    // The title is deliberately free of the measured value: it is what the
    // NOT EXISTS clause dedupes on, so a site that is slow for an hour raises
    // one alert rather than one per check. The varying figure lives in the
    // message, which is not part of the key.
    if new_status == "up" && response_time > 5000 {
        // Dedup keys on `state_key` (the monitor's id) rather than on the
        // title, because the title carries the monitor's NAME: renaming a
        // slow monitor used to raise a second firing alert for the same
        // subject, and the rename made the first one unreachable.
        //
        // This guard-then-fire split replaced a single atomic
        // `INSERT ... WHERE NOT EXISTS` — safe because `check_monitor` never
        // runs twice concurrently for the same monitor (the tick's JoinSet is
        // fully drained before the next monitor query), so there is no real
        // TOCTOU window between the two statements below.
        //
        // Firing now goes through `notifications::try_fire_alert` instead of a
        // raw INSERT: the raw form only ever wrote the `alerts` row, so a
        // firing slow-response alert got no bell/SSE entry, no email/Slack/
        // Discord delivery and no escalation paging — and could not honour the
        // per-type mute either, even though "slow_response" has been listed in
        // SUPPRESSIBLE_ALERT_TYPES as a mutable type since it was added. The
        // recovery half below already went through `resolve_alert`; this
        // brings the firing half to the same standard.
        let state_key = monitor.id.to_string();
        let already_firing: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM alerts \
             WHERE alert_type = 'slow_response' AND status = 'firing' \
               AND user_id = $1 AND state_key = $2)",
        )
        .bind(monitor.user_id)
        .bind(&state_key)
        .fetch_one(pool)
        .await
        .unwrap_or(false);

        if !already_firing {
            if let Err(e) = crate::services::notifications::try_fire_alert(
                pool,
                monitor.user_id,
                None,
                monitor.site_id,
                "slow_response",
                &state_key,
                "warning",
                &format!("Slow response: {}", monitor.name),
                &format!("Response time {}ms exceeds 5000ms threshold for {}", response_time, monitor.url),
            )
            .await
            {
                tracing::error!("Failed to record slow-response alert for {}: {e}", monitor.name);
            }
        }

        tracing::warn!("Monitor {} ({}) slow response: {}ms", monitor.name, monitor.url, response_time);
        crate::services::system_log::log_event(
            pool,
            "warning",
            "uptime",
            &format!("Slow response: {} ({}ms)", monitor.name, response_time),
            Some(&format!("URL: {}, threshold: 5000ms", monitor.url)),
        ).await;
    } else if new_status == "up" {
        // The recovery half, absent since the alert was first written.
        //
        // A firing row with no resolve path is worse than no alert at all: the
        // dedup guard above reads it and suppresses every subsequent
        // slow-response alert for this monitor, so the one stuck row also
        // blinds the check that created it. Meanwhile it stays visible to
        // `check_escalations` through `idx_alerts_escalation_sweep`
        // (status = 'firing' AND acknowledged_at IS NULL), which keeps paging
        // on-call about a site that got faster hours ago.
        //
        // `resolve_alert` no-ops when nothing was firing, so the common case —
        // a fast site staying fast — costs one indexed UPDATE returning zero
        // rows and sends nothing.
        crate::services::notifications::resolve_alert(
            pool,
            monitor.user_id,
            None,
            monitor.site_id,
            "slow_response",
            &monitor.id.to_string(),
            &format!("Slow response resolved: {}", monitor.name),
            &format!(
                "Response time is back to {}ms, under the 5000ms threshold, for {}",
                response_time, monitor.url
            ),
        )
        .await;
    }

    // Handle status transitions
    if new_status == "down" && monitor.status != "down" {
        // Just went down — create incident and send alerts
        let cause = error.as_deref().unwrap_or("Unknown error");
        if let Err(e) = sqlx::query(
            "INSERT INTO incidents (monitor_id, cause, alerted) VALUES ($1, $2, TRUE)",
        )
        .bind(monitor.id)
        .bind(cause)
        .execute(pool)
        .await {
            tracing::error!("Failed to create incident for {}: {e}", monitor.name);
        }

        tracing::warn!("Monitor {} ({}) is DOWN: {}", monitor.name, monitor.url, cause);
        crate::services::system_log::log_event(
            pool,
            "warning",
            "uptime",
            &format!("Monitor down: {} ({})", monitor.name, monitor.url),
            Some(cause),
        ).await;
        send_alerts(pool, monitor, &format!("{} is down: {cause}", monitor.name), "critical").await;

        // GAP 3: Auto-create managed incident for status page
        let _ = create_auto_incident(pool, monitor, cause).await;

        // GAP 19: Notify status page subscribers
        notify_status_subscribers(&monitor.name, "investigating", &format!("{} is experiencing issues: {cause}", monitor.name));
    } else if new_status == "up" && monitor.status == "down" {
        // Just recovered — resolve incident
        if let Err(e) = sqlx::query(
            "UPDATE incidents SET resolved_at = NOW() \
             WHERE monitor_id = $1 AND resolved_at IS NULL",
        )
        .bind(monitor.id)
        .execute(pool)
        .await {
            tracing::error!("Failed to resolve incident for {}: {e}", monitor.name);
        }

        // GAP 3: Auto-resolve managed incident
        let _ = resolve_auto_incident(pool, monitor).await;

        // GAP 19: Notify subscribers of recovery
        notify_status_subscribers(&monitor.name, "resolved", &format!("{} is back online", monitor.name));

        tracing::info!("Monitor {} ({}) is back UP", monitor.name, monitor.url);
        send_alerts(pool, monitor, &format!("{} is back up ({}ms)", monitor.name, response_time), "info").await;
    }
}

/// TCP port check — connect to host:port with timeout.
async fn check_tcp(monitor: &MonitorRow) -> (Option<i32>, Option<String>, &'static str, i32) {
    let host = monitor.url.trim_start_matches("tcp://");
    let port = monitor.port.unwrap_or(80) as u16;
    // SSRF re-validation at check time (parity with check_http): a low-TTL record can
    // flip to an internal IP after write, and rows imported via /settings/import bypass
    // the create-path guard. An internal target must not be probed.
    //
    // Resolved and pinned to the SAME address: dialing `host:port` again below
    // would let the OS resolve it a second time, independently of the check
    // just run against it — a rebinding DNS server (or simply a low TTL)
    // could then answer this second lookup with an internal address the
    // check above never saw.
    let addr = match crate::helpers::resolve_validated(host, port).await {
        Ok(addr) => addr,
        Err(e) => return (None, Some(format!("Host blocked: {e}")), "down", 0),
    };

    let start = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::TcpStream::connect(addr),
    ).await;
    let response_time = start.elapsed().as_millis() as i32;

    match result {
        Ok(Ok(_)) => (Some(0), None, "up", response_time),
        Ok(Err(e)) => (None, Some(format!("TCP connection failed: {e}")), "down", response_time),
        Err(_) => (None, Some("TCP connection timed out".to_string()), "down", response_time),
    }
}

/// Ping/ICMP check — uses system ping command.
async fn check_ping(monitor: &MonitorRow) -> (Option<i32>, Option<String>, &'static str, i32) {
    let host = monitor.url.trim_start_matches("ping://");
    // SSRF re-validation at check time (parity with check_http/check_tcp): reachability
    // of an internal host is itself the disclosure a ping monitor would leak.
    //
    // Pinned to the checked address, same reasoning as check_tcp: `ping` would
    // otherwise resolve `host` itself, a second and independent lookup from
    // the one just validated.
    let addr = match crate::helpers::resolve_validated(host, 0).await {
        Ok(addr) => addr,
        Err(e) => return (None, Some(format!("Host blocked: {e}")), "down", 0),
    };
    let target = addr.ip().to_string();
    let start = Instant::now();

    let output = tokio::time::timeout(
        Duration::from_secs(10),
        safe_command("ping")
            .args(["-c", "1", "-W", "5", &target])
            .output()
    ).await;

    let response_time = start.elapsed().as_millis() as i32;

    match output {
        Ok(Ok(o)) if o.status.success() => {
            // Parse response time from ping output: "time=X.XX ms"
            let stdout = String::from_utf8_lossy(&o.stdout);
            let ping_time = stdout.split("time=").nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<f64>().ok())
                .map(|ms| ms as i32)
                .unwrap_or(response_time);
            (Some(0), None, "up", ping_time)
        }
        _ => (None, Some("Ping failed or timed out".to_string()), "down", response_time),
    }
}

/// Heartbeat (dead man's switch) — alerts if no ping received within 2x interval.
async fn check_heartbeat(monitor: &MonitorRow, pool: &PgPool) {
    let expected_interval = Duration::from_secs(monitor.check_interval.max(60) as u64);
    let last_check = monitor.last_checked_at.unwrap_or_else(chrono::Utc::now);
    let elapsed = chrono::Utc::now() - last_check;

    let max_silence = chrono::Duration::from_std(expected_interval * 2)
        .unwrap_or(chrono::Duration::minutes(10));

    if elapsed > max_silence {
        // Missed heartbeat
        if monitor.status != "down" {
            if let Err(e) = sqlx::query(
                "INSERT INTO monitor_checks (monitor_id, status_code, response_time, error) VALUES ($1, NULL, 0, $2)",
            )
            .bind(monitor.id)
            .bind("Heartbeat missed")
            .execute(pool)
            .await {
                tracing::error!("Failed to insert heartbeat miss check for {}: {e}", monitor.name);
            }

            if let Err(e) = sqlx::query(
                "UPDATE monitors SET status = 'down', last_checked_at = NOW(), last_response_time = 0, last_status_code = NULL WHERE id = $1",
            )
            .bind(monitor.id)
            .execute(pool)
            .await {
                tracing::error!("Failed to update heartbeat monitor status for {}: {e}", monitor.name);
            }

            if let Err(e) = sqlx::query(
                "INSERT INTO incidents (monitor_id, cause, alerted) VALUES ($1, $2, TRUE)",
            )
            .bind(monitor.id)
            .bind("Heartbeat missed — no ping received")
            .execute(pool)
            .await {
                tracing::error!("Failed to create heartbeat incident for {}: {e}", monitor.name);
            }

            tracing::warn!("Monitor {} ({}) heartbeat missed", monitor.name, monitor.url);
            crate::services::system_log::log_event(
                pool,
                "warning",
                "uptime",
                &format!("Heartbeat missed: {} ({})", monitor.name, monitor.url),
                Some("No heartbeat received within expected interval"),
            ).await;
            send_alerts(pool, monitor, &format!("{} heartbeat missed — no ping received", monitor.name), "critical").await;
        }
    }
}

/// Describe a failed HTTP check without republishing the monitor's own URL.
///
/// `reqwest::Error`'s `Display` ends with ` for url ({url})` whenever the URL is
/// known, so `e.to_string()` on a connection failure carries the monitor's
/// address, path and query string verbatim. That one string becomes three
/// stored columns — `incidents.cause`, `managed_incidents.description`, and the
/// `Auto-detected: …` timeline entry — and `/api/status-page/public` serves the
/// first and third of them to anyone on the internet with no login. A monitor
/// URL routinely carries a token in its query string, `docs/guides/status-page.md`
/// states that URLs are not published, and the other public handler drops the
/// URL deliberately (`routes/monitors.rs::status_page` destructures it to `_url`).
///
/// `Error::without_url()` alone is not the repair. For a request-kind error it
/// leaves the four words `error sending request`, which is the whole message —
/// every failed HTTP check would read identically and no test would notice.
/// What an operator needs is the source chain (`Connection refused (os error
/// 111)`, `invalid peer certificate`), so this keeps that and drops only the
/// URL. The chain is walked rather than read one level down because the useful
/// sentence is the operating system's, three or four layers in.
///
/// The final scrub is not redundant with `without_url`, and the case that makes
/// it load-bearing was found by measurement rather than reasoning. Three failure
/// classes were driven against reqwest 0.12.28 (2026-08-23):
///
/// - connection refused → `client error (Connect)` → `Connection refused (os
///   error 111)`. No host anywhere. The scrub does not fire.
/// - DNS failure → `dns error` → `failed to lookup address information: Name or
///   service not known`. No host either — the obvious guess, and it is wrong.
/// - **TLS hostname mismatch → `invalid peer certificate: certificate not valid
///   for name "wrong.host.badssl.com"`.** The host, verbatim, in a source layer
///   that `without_url()` never touches. This is the one that needs the scrub.
///
/// ⚠ Residual, measured and deliberate: that same message goes on to name the
/// *certificate's* declared domains (`DnsName("*.badssl.com")`), which the scrub
/// does not remove because they are a property of the certificate the target
/// presented, not of the URL the operator configured. Truncating rustls's
/// sentence would mean string-matching a dependency's prose, which is the class
/// of mistake this whole function exists to undo.
fn describe_check_error(e: reqwest::Error, url: &str) -> String {
    let e = e.without_url();
    let mut msg = e.to_string();

    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&e);
    let mut depth = 0;
    while let Some(cause) = source {
        if depth >= 4 {
            break;
        }
        let text = cause.to_string();
        if !text.is_empty() && !msg.contains(&text) {
            msg.push_str(": ");
            msg.push_str(&text);
        }
        source = std::error::Error::source(cause);
        depth += 1;
    }

    redact_monitor_target(msg, url)
}

/// Remove a monitor's own address from a message about that monitor.
///
/// Three needles, longest first: the URL as the operator wrote it, the URL as
/// `url::Url` normalises it (a bare `http://host` is stored without the trailing
/// slash and printed with one), and the bare host. Longest-first matters — the
/// host is a substring of both URLs, so replacing it first would leave the path
/// and query stranded beside a redaction marker.
fn redact_monitor_target(mut msg: String, url: &str) -> String {
    let parsed = url::Url::parse(url).ok();
    let mut needles: Vec<String> = vec![url.trim().to_string()];
    if let Some(u) = &parsed {
        needles.push(u.to_string());
        if let Some(host) = u.host_str() {
            needles.push(host.to_string());
        }
    }
    needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
    for needle in needles {
        // Four characters is the shortest thing worth calling an address. Below
        // that a needle is likelier to be a fragment of an unrelated word, and
        // redacting it would corrupt the sentence it appears in.
        if needle.len() >= 4 && msg.contains(&needle) {
            msg = msg.replace(&needle, "(url withheld)");
        }
    }
    msg
}

/// The `reqwest::Client` settings every HTTP monitor check shares: a 30s
/// timeout and a redirect policy that blocks a hop into an internal address.
/// Extracted so `check_http` can build a FRESH client per call (below) with
/// these same settings, rather than duplicating them.
fn http_check_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        // Follow up to 5 redirects (http->https etc.) but NEVER to an internal address —
        // closes the redirect-to-internal SSRF/oracle bypass that a bare Policy::limited
        // would follow. The target host is fully resolved (literal IPs AND hostnames that
        // resolve to an internal IP are rejected); an over-limit chain surfaces as a
        // network error (→ "down") rather than a spurious 3xx.
        //
        // This blocking check catches a literal-IP redirect target outright (hyper's
        // connector dials a literal IP directly — it never asks a resolver at all, so a
        // resolver-level guard alone cannot see it) and gives hostname redirects a fast,
        // synchronous first pass. It does NOT by itself close the TOCTOU between "this
        // hostname checked clean" and "reqwest connects", which is why `.dns_resolver`
        // below installs `ValidatingResolver` as well: for a hostname redirect, THAT is
        // the lookup reqwest actually connects with, so there is nothing left to race.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 {
                return attempt.error("too many redirects");
            }
            let host = attempt.url().host_str().unwrap_or("").to_string();
            let port = attempt.url().port_or_known_default().unwrap_or(80);
            if crate::helpers::host_resolves_internal_blocking(&host, port) {
                attempt.error("redirect to internal address blocked")
            } else {
                attempt.follow()
            }
        }))
        // Fallback resolver for every hostname reqwest resolves that ISN'T the initial
        // request's (that one is pinned separately by `pinned_client`'s `.resolve()`
        // override, checked first) — covers every redirect hop to a different host.
        .dns_resolver(std::sync::Arc::new(crate::helpers::ValidatingResolver))
        .danger_accept_invalid_certs(false)
}

/// HTTP check with optional keyword verification and custom headers.
///
/// `_client` is the tick's shared, pooled client — kept as a parameter since
/// `check_monitor` still threads it through for the other check kinds, but no
/// longer used HERE for the actual request. A shared client's resolver would
/// re-resolve `monitor.url`'s host independently of the SSRF check just run
/// against it; this function instead pins the request to the EXACT address
/// that check approved, via a fresh per-call client (`pinned_client`) built
/// with the same settings (`http_check_client_builder`).
async fn check_http(monitor: &MonitorRow, _client: &reqwest::Client) -> (Option<i32>, Option<String>, &'static str, i32) {
    let (host, port) = match crate::helpers::url_authority(&monitor.url) {
        Ok(hp) => hp,
        Err(e) => return (None, Some(format!("URL blocked: {e}")), "down", 0),
    };
    // SSRF re-validation at check time: the URL was vetted at write time, but a low-TTL
    // DNS record can flip to an internal IP before the check runs (rebind), and monitors
    // imported via /settings/import bypass the create-path guard entirely. Done BEFORE the
    // timing window so its DNS lookup does not inflate the recorded response_time.
    let client = match crate::helpers::pinned_client(&host, port, http_check_client_builder()).await {
        Ok(c) => c,
        Err(e) => return (None, Some(format!("URL blocked: {e}")), "down", 0),
    };
    let start = Instant::now();
    let mut builder = client.get(&monitor.url);

    // Apply custom headers if present
    if let Some(ref headers_json) = monitor.custom_headers {
        if let Some(headers_map) = headers_json.as_object() {
            for (key, value) in headers_map {
                if let Some(v) = value.as_str() {
                    if let (Ok(name), Ok(val)) = (
                        reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                        reqwest::header::HeaderValue::from_str(v),
                    ) {
                        builder = builder.header(name, val);
                    }
                }
            }
        }
    }

    let result = builder.send().await;
    let response_time = start.elapsed().as_millis() as i32;

    match result {
        Ok(mut resp) => {
            let code = resp.status().as_u16() as i32;
            if !resp.status().is_success() {
                return (Some(code), Some(format!("HTTP {code}")), "down", response_time);
            }

            // Keyword check if configured
            if let Some(ref keyword) = monitor.keyword {
                if !keyword.is_empty() {
                    // Bound the body read: an attacker-controlled target could otherwise
                    // stream an unbounded body into memory (shared-process OOM on small
                    // VPSes; the agent proxy caps responses the same way via
                    // http_body_util::Limited). 2 MiB covers real HTML/JSON pages while
                    // capping peak memory at ~2 MiB × 10 concurrent checks. Keyword match is
                    // UTF-8/ASCII (from_utf8_lossy); an ASCII keyword always matches, a
                    // non-ASCII keyword on a non-UTF-8 page may not.
                    const MAX_BODY: usize = 2 * 1024 * 1024;
                    let mut buf: Vec<u8> = Vec::new();
                    while let Ok(Some(chunk)) = resp.chunk().await {
                        let take = (MAX_BODY - buf.len()).min(chunk.len());
                        buf.extend_from_slice(&chunk[..take]);
                        if buf.len() >= MAX_BODY {
                            break;
                        }
                    }
                    let body = String::from_utf8_lossy(&buf).to_string();
                    let contains = body.contains(keyword.as_str());
                    let must_contain = monitor.keyword_must_contain;

                    if (must_contain && !contains) || (!must_contain && contains) {
                        let error = if must_contain {
                            format!("Keyword '{}' not found in response", keyword)
                        } else {
                            format!("Keyword '{}' found in response (should not be present)", keyword)
                        };
                        return (Some(code), Some(error), "down", response_time);
                    }
                }
            }

            (Some(code), None, "up", response_time)
        }
        Err(e) => (None, Some(describe_check_error(e, &monitor.url)), "down", response_time),
    }
}

/// Deliver a monitor state change to the owner's external channels AND to the
/// panel's own notification centre.
///
/// The panel half was missing entirely: this module had zero `notify_panel`
/// calls, so a monitored site going down reached email, Slack, Discord and
/// PagerDuty — every destination that lives outside the product — and never the
/// bell inside it. An operator watching the panel while a site fell over saw
/// nothing change. The external channels here are per-monitor (`alert_email`,
/// `alert_slack_url`, `alert_discord_url`), so they are not subject to the
/// missing-`alert_rules` gate that silences the security alerts; this is a
/// straightforward omission rather than a fail-closed default.
async fn send_alerts(pool: &PgPool, monitor: &MonitorRow, message: &str, severity: &str) {
    // Build notification channels from monitor's per-monitor settings
    let email = if monitor.alert_email {
        sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id = $1")
            .bind(monitor.user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    // Get PagerDuty key from alert_rules
    let extra_channels: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT notify_pagerduty_key, notify_webhook_url FROM alert_rules WHERE user_id = $1 AND server_id IS NULL"
    ).bind(monitor.user_id).fetch_optional(pool).await.ok().flatten();

    let (pagerduty_key, webhook_url) = extra_channels.unwrap_or((None, None));

    let channels = crate::services::notifications::NotifyChannels {
        email,
        slack_url: monitor.alert_slack_url.clone(),
        discord_url: monitor.alert_discord_url.clone(),
        pagerduty_key,
        webhook_url,
        muted_types: String::new(),
    };

    let subject = format!("DockPanel Alert: {}", monitor.name);
    let html = format!(
        "<h2>Monitor Alert</h2>\
         <p><strong>{}</strong></p>\
         <p>URL: {}</p>\
         <p>Time: {}</p>",
        message,
        monitor.url,
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
    );

    crate::services::notifications::send_notification(pool, &channels, &subject, message, &html)
        .await;

    // Addressed to the monitor's owner rather than broadcast to every admin:
    // `monitors.user_id` is who asked to be told, and it is the same identity
    // the external channels above are built from.
    crate::services::notifications::notify_panel(
        pool,
        Some(monitor.user_id),
        &subject,
        message,
        severity,
        "monitor",
        Some("/monitoring?tab=monitors"),
    )
    .await;
}

/// GAP 3: Auto-create a managed incident when a monitor goes down.
async fn create_auto_incident(pool: &PgPool, monitor: &MonitorRow, cause: &str) -> Result<(), String> {
    // Find admin user for the monitor
    let user: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT user_id FROM monitors WHERE id = $1"
    )
    .bind(monitor.id)
    .fetch_optional(pool).await.ok().flatten();

    let user_id = match user {
        Some((id,)) => id,
        None => return Ok(()),
    };

    // Create managed incident
    let incident_id: uuid::Uuid = match sqlx::query_scalar(
        "INSERT INTO managed_incidents (user_id, title, status, severity, description, visible_on_status_page) \
         VALUES ($1, $2, 'investigating', 'major', $3, TRUE) RETURNING id"
    )
    .bind(user_id)
    .bind(format!("{} is down", monitor.name))
    .bind(cause)
    .fetch_one(pool).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("Failed to create managed incident: {e}");
            return Err(e.to_string());
        }
    };

    // Create initial update
    let _ = sqlx::query(
        "INSERT INTO incident_updates (incident_id, status, message, author_email) \
         VALUES ($1, 'investigating', $2, 'system')"
    )
    .bind(incident_id)
    .bind(format!("Auto-detected: {cause}"))
    .execute(pool).await;

    // Link to status page components via monitor
    let _ = sqlx::query(
        "INSERT INTO managed_incident_components (incident_id, component_id) \
         SELECT $1, cm.component_id FROM status_page_component_monitors cm WHERE cm.monitor_id = $2 \
         ON CONFLICT DO NOTHING"
    )
    .bind(incident_id).bind(monitor.id)
    .execute(pool).await;

    tracing::info!("Auto-incident created for monitor {} (incident {})", monitor.name, incident_id);
    Ok(())
}

/// GAP 3: Auto-resolve managed incident when monitor recovers.
async fn resolve_auto_incident(pool: &PgPool, monitor: &MonitorRow) -> Result<(), String> {
    // Find unresolved managed incidents with matching title pattern, scoped to
    // the monitor's owner. Monitor names are per-tenant and unremarkable ("api",
    // "website"), so matching on title alone let one tenant's monitor recovering
    // resolve another tenant's still-open incident and post a "recovered
    // automatically" update onto their public status page.
    let incidents: Vec<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM managed_incidents \
         WHERE title = $1 AND user_id = $2 AND status != 'resolved' AND status != 'postmortem'"
    )
    .bind(format!("{} is down", monitor.name))
    .bind(monitor.user_id)
    .fetch_all(pool).await.unwrap_or_default();

    for (incident_id,) in &incidents {
        // Post resolved update
        let _ = sqlx::query(
            "INSERT INTO incident_updates (incident_id, status, message, author_email) \
             VALUES ($1, 'resolved', 'Monitor recovered automatically', 'system')"
        )
        .bind(incident_id)
        .execute(pool).await;

        // Resolve the incident
        let _ = sqlx::query(
            "UPDATE managed_incidents SET status = 'resolved', resolved_at = NOW(), updated_at = NOW() WHERE id = $1"
        )
        .bind(incident_id)
        .execute(pool).await;
    }

    if !incidents.is_empty() {
        tracing::info!("Auto-resolved {} managed incident(s) for monitor {}", incidents.len(), monitor.name);
    }
    Ok(())
}

/// GAP 19: Notify status page subscribers of monitor events.
///
/// Hands off to the shared status-notice worker — see
/// `services::status_notices` for why this must not run inline in the monitor
/// check and must not be a detached per-event spawn.
fn notify_status_subscribers(monitor_name: &str, status: &str, message: &str) {
    crate::services::status_notices::enqueue(
        monitor_name,
        format!("[Status Update] {} — {}", monitor_name, status),
        message.to_string(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deliberately credential-shaped. The reason `describe_check_error` exists
    /// is that a health endpoint's query string is where people put tokens.
    const SECRET_URL: &str = "http://127.0.0.1:9/internal/health?api_key=SUPERSECRET123";

    async fn refused_error() -> reqwest::Error {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("client builds")
            .get(SECRET_URL)
            .send()
            .await
            .expect_err("port 9 on loopback must refuse the connection")
    }

    #[tokio::test]
    async fn a_failed_check_does_not_republish_the_monitor_url() {
        let e = refused_error().await;

        // POSITIVE CONTROL, and it is load-bearing: this is the exact string the
        // panel used to store and publish. If a future reqwest stops printing
        // the URL, this assertion fails loudly rather than letting the three
        // below start passing for a reason that has nothing to do with the fix.
        let raw = e.to_string();
        assert!(
            raw.contains("SUPERSECRET123"),
            "control failed — reqwest no longer prints the URL, so this test proves nothing: {raw}"
        );

        let described = describe_check_error(e, SECRET_URL);
        assert!(
            !described.contains("SUPERSECRET123"),
            "query string published: {described}"
        );
        assert!(
            !described.contains("internal/health"),
            "path published: {described}"
        );
        assert!(
            !described.contains("127.0.0.1"),
            "host published: {described}"
        );

        // ...and it still says something. `without_url()` on its own leaves the
        // four words "error sending request" for every failure there is.
        assert!(
            described.len() > "error sending request".len(),
            "message reduced to nothing an operator can act on: {described}"
        );
    }

    /// The fixture is a MEASURED string, not an invented one. Driven against
    /// reqwest 0.12.28 + rustls on 2026-08-23 with a real hostname-mismatch
    /// target, this is the source-chain text verbatim — and it is the only one
    /// of the three failure classes that names the host, which is the whole
    /// reason the scrub exists. `without_url()` does not touch it, because the
    /// host is inside a source layer rather than in the error's URL slot.
    #[test]
    fn the_scrub_removes_the_host_a_tls_mismatch_prints() {
        let out = redact_monitor_target(
            "error sending request: client error (Connect): invalid peer certificate: \
             certificate not valid for name \"status.example.com\"; certificate is only \
             valid for DnsName(\"*.other.example\")"
                .to_string(),
            "https://status.example.com/healthz?token=abc",
        );
        assert!(
            !out.contains("status.example.com"),
            "host survived the scrub: {out}"
        );
        assert!(
            out.contains("invalid peer certificate"),
            "the operator's half was destroyed: {out}"
        );
    }

    #[test]
    fn the_scrub_takes_the_whole_url_before_the_bare_host() {
        // Longest-first is the whole point: the host is a substring of the URL,
        // so redacting it first would strand the path and query beside the marker.
        let out = redact_monitor_target(
            "error sending request for url (https://status.example.com/healthz?token=abc)"
                .to_string(),
            "https://status.example.com/healthz?token=abc",
        );
        assert!(!out.contains("token=abc"), "query survived: {out}");
        assert!(!out.contains("healthz"), "path survived: {out}");
    }
}
