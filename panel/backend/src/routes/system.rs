use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures::stream::StreamExt;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser, ServerScope};
use crate::error::{agent_error, ApiError};
use crate::routes::sites::ProvisionStep;
use crate::services::activity;
use crate::services::agent::AgentHandle;
use crate::AppState;

/// GET /api/health — Public health check (includes DB connectivity).
pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let db_ok = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();

    if db_ok {
        Json(serde_json::json!({
            "status": "ok",
            "service": "dockpanel-api",
            "version": env!("CARGO_PKG_VERSION"),
        }))
    } else {
        Json(serde_json::json!({
            "status": "degraded",
            "db": "unreachable",
            "service": "dockpanel-api",
            "version": env!("CARGO_PKG_VERSION"),
        }))
    }
}

/// GET /api/system/info — Proxy to agent's system info (admin only).
pub async fn info(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/system/info")
        .await
        .map_err(|e| agent_error("System info", e))?;
    Ok(Json(data))
}

/// GET /api/agent/diagnostics — Proxy to agent's diagnostics (admin only).
pub async fn diagnostics(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/diagnostics")
        .await
        .map_err(|e| agent_error("Diagnostics", e))?;
    Ok(Json(data))
}

/// POST /api/agent/diagnostics/fix — Apply a one-click fix.
///
/// Mostly a proxy to the agent, with ONE fix the agent cannot perform and the
/// panel can.
///
/// **`renew-ssl:{domain}` was a button that could only fail.** The agent's
/// diagnostics offers it on every certificate inside 30 days
/// (`diagnostics.rs::check_ssl_expiry`), but `apply_fix` has no arm for it, so it
/// fell through to `Unknown fix action` — which the agent's route maps to
/// **500**, and `agent_error` preserves an agent's own sentence only for 4xx. So
/// the operator got `Operation failed. Reference: {uuid}`: indistinguishable
/// from a transient fault, which invites a retry, and every click wrote a
/// `tracing::error!` incident describing a working agent as broken.
///
/// The agent cannot fix this itself and never could — renewal needs the site's
/// runtime, root, PHP version and an ACME contact, all of which live in the
/// panel's database, and an agent has no database. So the panel takes the fix.
/// Doing it here rather than in the agent is also what makes it ARRIVE:
/// `security_scanner` already records the same reasoning — "an agent is only
/// updated when somebody updates it… declining here is what makes the fix arrive
/// with the PANEL".
///
/// A certificate under `/etc/dockpanel/ssl` with no `sites` row — a DNS-01
/// wildcard apex, or one placed by hand — now gets a sentence saying so instead
/// of an incident id. That is still a refusal, but it is an ANSWER.
pub async fn diagnostics_fix(
    State(state): State<AppState>,
    crate::auth::AdminUser(claims): crate::auth::AdminUser,
    ServerScope(server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let fix_id = body.get("fix_id").and_then(|v| v.as_str()).unwrap_or("").to_string();

    if let Some(domain) = fix_id.strip_prefix("renew-ssl:") {
        // Resolution is two statements on purpose, and it lives HERE rather than
        // behind a helper because this is the handler that chooses a destination:
        // a reader — and the wrong-host-dispatch census — must be able to see the
        // host being named without following a call.
        //
        // The first statement names the row ON THE HOST THAT RAISED THE FINDING.
        // `sites.domain` is unique only per server (`idx_sites_domain_server`), so
        // a lookup on the name alone can hand back a different machine's site and
        // this handler would renew the wrong one — the same defect this ship fixes
        // in `security_scanner`. The second re-reads it through
        // `SITE_CALLER_PREDICATE`, the shared authorisation every other site
        // handler uses, so this new door decides nothing about who may open it:
        // reuse carries the mechanism, never the gate.
        let site_id: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM sites WHERE domain = $1 AND server_id = $2",
        )
        .bind(domain)
        .bind(server_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| crate::error::internal_error("diagnostics ssl fix lookup", e))?;

        let Some((site_id,)) = site_id else {
            // A certificate under /etc/dockpanel/ssl with no `sites` row. Still a
            // refusal, but a 4xx with a sentence, so `agent_error`'s sibling rule
            // cannot later collapse it into an incident id the way the agent's 500
            // did.
            //
            // ⚠ It must NOT claim the certificate came from outside DockPanel,
            // because very often it did not: the agent issues certificates for
            // Docker apps, Compose stacks, Git deploys and mail domains, and NONE
            // of those becomes a `sites` row — a Docker app has no table at all, it
            // exists only as a labelled container. Saying "whatever put it there is
            // what renews it" to an operator whose own panel put it there is the
            // same species of defect as the button this branch replaced. So the
            // refusal names what the panel actually found, and stops there.
            //
            // These lookups are deliberately not server-scoped. `mail_domains.server_id`
            // was NULL on every panel-created row for five months and
            // `git_deploys.domain` is nullable, so a server term here would turn a
            // true "a mail domain claims this name" into a silent "nothing does" —
            // trading a wrong sentence for a wronger one. Naming the owner
            // approximately is worth more than naming nothing precisely.
            let claimed_by: Option<(String,)> = sqlx::query_as(
                "SELECT 'mail domain' FROM mail_domains WHERE domain = $1 \
                 UNION ALL SELECT 'Git deploy' FROM git_deploys WHERE domain = $1 \
                 UNION ALL SELECT 'Docker app' FROM container_sleep_config WHERE domain = $1 \
                 UNION ALL SELECT 'Compose stack' FROM docker_stacks WHERE lower(domain) = lower($1) \
                 LIMIT 1",
            )
            .bind(domain)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

            // A stack is the one claimant whose answer depends on its TLS MODE.
            // "DockPanel issued this and a redeploy reissues it" is true of an
            // `acme` stack and FALSE of a `provided` one, whose live certificate
            // is the operator's own, served from the registry — the expiring
            // file this button was clicked about is then a leftover under the
            // per-domain tree, and redeploying reissues nothing. Telling that
            // operator to redeploy is the same species of wrong sentence this
            // whole branch exists to stop.
            //
            // ⛔ Read through `effective_tls_mode`, never a CASE in the SQL
            // above: the NULL⇒ssl_email rule has ONE spelling, and a second one
            // here would drift exactly where it decides what to tell an
            // operator about their own certificate. Unscoped by server for the
            // same reason as the claimant probe above.
            let stack_mode = if claimed_by.as_ref().map(|(k,)| k.as_str()) == Some("Compose stack") {
                let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
                    "SELECT tls_mode, ssl_email FROM docker_stacks WHERE lower(domain) = lower($1)",
                )
                .bind(domain)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None);
                row.map(|(m, e)| {
                    crate::routes::stacks::effective_tls_mode(m.as_deref(), e.as_deref())
                })
            } else {
                None
            };

            let tail = match claimed_by {
                Some((_,)) if matches!(stack_mode, Some(mode) if mode != "acme") => {
                    "A Compose stack in this panel uses that name, but it serves a registered \
                     certificate rather than a Let's Encrypt one — so this expiring file is a \
                     leftover from before that change, and nothing renews it because nothing \
                     serves it. Removing it is safe; the stack is unaffected."
                        .to_string()
                }
                Some((kind,)) => format!(
                    "A {kind} in this panel uses that name, and DockPanel issued this \
                     certificate for it — but only certificates attached to a SITE can be \
                     renewed from here. Redeploy that {kind} to reissue it."
                ),
                None => "No site, mail domain, Git deploy, Docker app or Compose stack in \
                         this panel claims that name — so this is a certificate DockPanel \
                         does not manage, such as a DNS-01 wildcard or one installed by \
                         hand, and it is renewed wherever it was issued."
                    .to_string(),
            };

            return Err(crate::error::err(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("No site named {domain} is registered on this server. {tail}"),
            ));
        };

        let site: crate::models::Site = sqlx::query_as(&format!(
            "SELECT s.* FROM sites s WHERE {}",
            crate::helpers::SITE_CALLER_PREDICATE
        ))
        .bind(site_id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| crate::error::internal_error("diagnostics ssl fix", e))?
        .ok_or_else(|| crate::error::err(StatusCode::NOT_FOUND, "Site not found"))?;

        // The renewal's own `{ok, domain}` body is deliberately dropped: this
        // endpoint answers the Diagnostics screen, which reads `{success, message}`.
        // The `?` is the load-bearing part — a PRECONDITION_FAILED from
        // `resolve_acme_contact`, or a 4xx the agent authored, reaches the operator
        // as that sentence instead of a reference id.
        let _renewed =
            crate::routes::ssl::renew_for_site(&state, &site, claims.sub, &claims.email).await?;

        activity::log_activity_on_server(
            &state.db, claims.sub, &claims.email, "diagnostics.fix",
            Some("renew-ssl"), Some(domain), None, None, Some(server_id),
        ).await;

        // The shape the Diagnostics screen reads.
        return Ok(Json(serde_json::json!({
            "success": true,
            "message": format!("Renewed the certificate for {domain}"),
        })));
    }

    let data = agent
        .post("/diagnostics/fix", Some(body))
        .await
        .map_err(|e| agent_error("Diagnostics fix", e))?;

    let (action, target) = match fix_id.split_once(':') {
        Some((a, t)) => (a.to_string(), Some(t.to_string())),
        None => (fix_id.clone(), None),
    };
    activity::log_activity_on_server(
        &state.db, claims.sub, &claims.email, "diagnostics.fix",
        Some(&action), target.as_deref(), None, None, Some(server_id),
    ).await;

    Ok(Json(data))
}

/// GET /api/agent/recommendations — Auto-optimization recommendations (admin only).
pub async fn recommendations(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/diagnostics/recommendations")
        .await
        .map_err(|e| agent_error("Recommendations", e))?;
    Ok(Json(data))
}

/// POST /api/system/cleanup — Proxy to agent's disk cleanup (admin only).
pub async fn disk_cleanup(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .post("/system/cleanup", None)
        .await
        .map_err(|e| agent_error("Disk cleanup", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "system.cleanup",
        None, None, None, None,
    ).await;

    Ok(Json(data))
}

/// POST /api/system/hostname — Proxy to agent's hostname change (admin only).
pub async fn change_hostname(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .post("/system/hostname", Some(body))
        .await
        .map_err(|e| agent_error("Hostname change", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "system.hostname_change",
        None, None, None, None,
    ).await;

    Ok(Json(data))
}

/// GET /api/system/updates — List available package updates (admin only).
pub async fn updates_list(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/system/updates")
        .await
        .map_err(|e| agent_error("System updates", e))?;
    Ok(Json(data))
}

/// How long an update stream may stay *silent* before it is declared wedged.
///
/// This is not a budget for the update — apt is allowed to take as long as it
/// takes. It only has to say something every ten minutes, which comfortably
/// covers the quietest legitimate stretches (unpacking a large package,
/// `Generating locales`, a slow mirror mid-download) while still failing a
/// genuinely hung run in bounded time.
const UPDATE_STREAM_IDLE_TIMEOUT_SECS: u64 = 600;

/// How long a finished update's log is kept for late SSE reconnects.
///
/// The browser replays the full history on reconnect, so this window is what
/// lets an operator who lost the connection — closed the laptop, changed
/// network, hit the tail of a service restart — come back and still find out
/// whether the update succeeded. At 60s it was shorter than the gap it needed
/// to cover.
const UPDATE_LOG_RETENTION_SECS: u64 = 900;

/// POST /api/system/updates/apply — Apply package updates (admin only).
/// Returns install_id for SSE progress tracking via /api/services/install/{id}/log.
/// Proxies to agent which runs apt with streaming NDJSON output, forwarded
/// line-by-line as SSE events for a live terminal experience.
pub async fn updates_apply(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let install_id = uuid::Uuid::new_v4();

    // 256: this one forwards apt output line by line, not a dozen coarse steps.
    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        install_id,
        claims.sub,
        256,
    );

    let logs = state.provision_logs.clone();
    let db = state.db.clone();
    let email = claims.email.clone();
    let user_id = claims.sub;

    tokio::spawn(async move {
        let emit = |step: &str, label: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(), label: label.into(), status: status.into(), message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&install_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        emit("update", "Applying system updates", "in_progress", None);

        // Use streaming NDJSON: agent sends each apt output line as it happens
        let logs_cb = logs.clone();
        let emit_line = move |json: serde_json::Value| {
            let ev_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match ev_type {
                "line" => {
                    let line = json.get("line").and_then(|v| v.as_str()).unwrap_or("");
                    if !line.is_empty() {
                        let ev = ProvisionStep {
                            step: "line".into(),
                            label: line.into(),
                            status: "in_progress".into(),
                            message: None,
                        };
                        if let Ok(mut map) = logs_cb.lock() {
                            if let Some((history, tx, _)) = map.get_mut(&install_id) {
                                history.push(ev.clone());
                                let _ = tx.send(ev);
                            }
                        }
                    }
                }
                "done" => {
                    let success = json.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                    let (update_status, complete_label, complete_status) = if success {
                        ("done", "Updates applied", "done")
                    } else {
                        ("error", "Updates finished with errors", "error")
                    };

                    for (step, label, status) in [
                        ("update", "Applying system updates", update_status),
                        ("complete", complete_label, complete_status),
                    ] {
                        let ev = ProvisionStep {
                            step: step.into(),
                            label: label.into(),
                            status: status.into(),
                            message: None,
                        };
                        if let Ok(mut map) = logs_cb.lock() {
                            if let Some((history, tx, _)) = map.get_mut(&install_id) {
                                history.push(ev.clone());
                                let _ = tx.send(ev);
                            }
                        }
                    }
                }
                _ => {}
            }
        };

        match agent
            .post_long_ndjson(
                "/system/updates/apply",
                Some(body),
                UPDATE_STREAM_IDLE_TIMEOUT_SECS,
                emit_line,
            )
            .await
        {
            Ok(()) => {
                activity::log_activity(&db, user_id, &email, "system.updates.apply",
                    Some("system"), Some("packages"), None, None).await;
            }
            Err(e) => {
                emit("update", "Failed to apply updates", "error", Some(format!("{e}")));
                emit("complete", "Update failed", "error", None);
            }
        }

        tokio::time::sleep(Duration::from_secs(UPDATE_LOG_RETENTION_SECS)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&install_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "install_id": install_id,
        "message": "Updates started",
    }))))
}

/// GET /api/system/updates/count — Get count of available updates (admin only).
pub async fn updates_count(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/system/updates/count")
        .await
        .map_err(|e| agent_error("Update count", e))?;
    Ok(Json(data))
}

/// POST /api/system/reboot — Reboot the system (admin only).
pub async fn system_reboot(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .post("/system/reboot", None::<serde_json::Value>)
        .await
        .map_err(|e| agent_error("System reboot", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "system.reboot",
        Some("system"), Some("server"), None, None,
    ).await;

    Ok(Json(data))
}

// ── Service installers (proxy to agent, async with SSE progress) ─────────

pub async fn install_status(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/services/install-status").await
        .map_err(|e| agent_error("Install status", e))?;
    Ok(Json(result))
}

/// How long the panel gives the agent to finish an install.
///
/// Deliberately far above the agent's own `INSTALL_TIMEOUT` (300s in
/// `services::pkg::transact`), because the agent does work either side of that
/// clock — refreshing the package index, adding a third-party repo — and the
/// budget has to cover the whole call, not the part that happens to be measured.
///
/// It used to be 60s, from the untimed `post`, over an operation the agent
/// budgets five minutes for. Nothing was cancelled by that: the agent finished
/// the install while this side had already written *"agent request timed out
/// after 60s"* into the log the operator was watching. A caller timeout shorter
/// than the callee's does not bound anything — it only decides which of the two
/// gets to describe the outcome, and picks the one that does not know it.
const INSTALL_AGENT_TIMEOUT_SECS: u64 = 900;

/// Generic service install with provisioning log (async SSE).
pub(crate) async fn install_service_with_log(
    state: &AppState,
    agent: AgentHandle,
    claims_sub: Uuid,
    claims_email: &str,
    service_name: &str,
    agent_path: &str,
    agent_body: Option<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let install_id = Uuid::new_v4();

    // One line here covers every service install and uninstall route plus both
    // PHP routes — they all funnel through this helper.
    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        install_id,
        claims_sub,
        32,
    );

    let logs = state.provision_logs.clone();
    let db = state.db.clone();
    let svc = service_name.to_string();
    let path = agent_path.to_string();
    let email = claims_email.to_string();

    tokio::spawn(async move {
        let emit = |step: &str, lbl: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(),
                label: lbl.into(),
                status: status.into(),
                message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&install_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        emit("install", &format!("Installing {svc}"), "in_progress", None);

        match agent
            .post_long(&path, agent_body, INSTALL_AGENT_TIMEOUT_SECS)
            .await
        {
            Ok(resp) => {
                // A 200 is not the same as a success, and for PHP it stopped being
                // the same the moment the agent started judging an install by
                // whether FPM opened its socket: "the packages are on disk but the
                // service never came up" is an honest 200 carrying
                // `success: false`. Reading only the status code would paint that
                // green with the explanation printed underneath it.
                //
                // Absent means success — most installers answer `{"ok": true}` or a
                // status object with no such field, and treating a missing field as
                // failure would turn every one of them red.
                let ok = resp
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                // The agent also answers 200 for "already installed" and carries the
                // distinction in `message`. Passing it through is the difference
                // between a log that says what happened and one that says it ended.
                let detail = resp
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                emit(
                    "install",
                    &format!("Installing {svc}"),
                    if ok { "done" } else { "error" },
                    detail.clone(),
                );
                if ok {
                    emit("complete", &format!("{svc} installed"), "done", detail);
                    tracing::info!("Service installed: {svc}");
                } else {
                    emit("complete", &format!("{svc} did not finish"), "error", detail);
                    tracing::warn!("Service install reported failure: {svc}");
                }
                // Logged either way: the operator asked for an install and one was
                // attempted, and an attempt that did not finish is the entry most
                // worth finding later.
                activity::log_activity(
                    &db, claims_sub, &email, "service.install",
                    Some("system"), Some(&svc), None, None,
                ).await;
            }
            Err(e) => {
                emit("install", &format!("Installing {svc}"), "error", Some(format!("{e}")));
                emit("complete", "Install failed", "error", None);
                tracing::error!("Service install failed: {svc}: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(30)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&install_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "install_id": install_id,
        "message": format!("{service_name} installation started"),
    }))))
}

pub async fn install_php(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "PHP", "/services/install/php", None).await
}

pub async fn install_certbot(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Certbot", "/services/install/certbot", None).await
}

pub async fn install_ufw(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "UFW Firewall", "/services/install/ufw", None).await
}

/// GET /api/services/install/{install_id}/log — SSE stream of install progress.
///
/// Despite the path, this is the panel's general provisioning stream: the UI
/// points backup ids, restore ids, site-deploy ids, mail-install ids and
/// system-update ids at it as well as service installs. So it cannot authorize
/// by feature — and it used to authorize by nothing at all. It took `AdminUser`,
/// discarded the claims, and looked the id up bare, which handed any key any
/// feature had put in the shared map to any admin: including another tenant's
/// site provisioning log, and the cleartext CMS admin password on it.
///
/// `AuthUser` in place of `AdminUser` is not a relaxation. Ownership is now
/// checked per key, which is strictly narrower than "anyone with the admin
/// role". It also repairs the opposite half of the same mistake: backups and
/// site deploys are owner-authorized routes open to any user, so a non-admin
/// who started one could never read back the log of the job they had just
/// launched — the stream 403'd the one person entitled to it.
pub async fn install_log(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(install_id): Path<Uuid>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, axum::BoxError>>>, ApiError> {
    let (snapshot, rx) = crate::helpers::open_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        install_id,
        claims.sub,
        "No active install",
    )?;

    let snapshot_stream = futures::stream::iter(
        snapshot.into_iter().map(|step| {
            let data = serde_json::to_string(&step).unwrap_or_default();
            Ok(Event::default().data(data))
        }),
    );

    let live_stream = BroadcastStream::new(rx).filter_map(|result| async {
        match result {
            Ok(step) => {
                let data = serde_json::to_string(&step).ok()?;
                Some(Ok(Event::default().data(data)))
            }
            Err(_) => None,
        }
    });

    Ok(
        Sse::new(snapshot_stream.chain(live_stream))
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping")),
    )
}

// ── SSH Keys ────────────────────────────────────────────────────────────

pub async fn list_ssh_keys(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/ssh-keys").await.map_err(|e| agent_error("SSH keys", e))?;
    Ok(Json(result))
}

pub async fn add_ssh_key(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.post("/ssh-keys", Some(body)).await.map_err(|e| agent_error("Add SSH key", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "ssh.key.add", Some("system"), None, None, None).await;
    Ok(Json(result))
}

pub async fn remove_ssh_key(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    axum::extract::Path(fingerprint): axum::extract::Path<String>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.delete(&format!("/ssh-keys/{fingerprint}")).await.map_err(|e| agent_error("Remove SSH key", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "ssh.key.remove", Some("system"), None, None, None).await;
    Ok(Json(result))
}

// ── Auto-Updates ────────────────────────────────────────────────────────

pub async fn auto_updates_status(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/auto-updates/status").await.map_err(|e| agent_error("Auto-updates", e))?;
    Ok(Json(result))
}

pub async fn enable_auto_updates(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.post("/auto-updates/enable", None).await.map_err(|e| agent_error("Enable auto-updates", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "auto-updates.enable", Some("system"), None, None, None).await;
    Ok(Json(result))
}

pub async fn disable_auto_updates(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.post("/auto-updates/disable", None).await.map_err(|e| agent_error("Disable auto-updates", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "auto-updates.disable", Some("system"), None, None, None).await;
    Ok(Json(result))
}

// The "Panel IP Whitelist" proxy that stood here was removed in v2.90.0. It relayed a
// list of addresses to the agent, which wrote them to a file on the agent host that
// nothing on any install ever read — no nginx include, no enforcement, nowhere. The
// panel's real IP restriction is a setting enforced at every session-minting door in
// `auth.rs`, and it now owns the single operator control on the Account tab.
//
pub async fn install_powerdns(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // Optional {"backend":"sqlite"|"pgsql"} forwarded to the agent (issue #63).
    // Absent/invalid → None, so the agent applies its pgsql default (back-compat).
    let forward_body = serde_json::from_slice::<serde_json::Value>(&body).ok()
        .and_then(|v| v.get("backend").and_then(|b| b.as_str()).map(str::to_string))
        .filter(|b| b == "sqlite" || b == "pgsql")
        .map(|b| serde_json::json!({ "backend": b }));
    let install_id = Uuid::new_v4();

    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        install_id,
        claims.sub,
        32,
    );

    let logs = state.provision_logs.clone();
    let db = state.db.clone();
    let jwt_secret = state.config.jwt_secret.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();

    tokio::spawn(async move {
        let emit = |step: &str, lbl: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(),
                label: lbl.into(),
                status: status.into(),
                message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&install_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        emit("install", "Installing PowerDNS", "in_progress", None);

        // `post_long` for the same reason as the mail installers — this is an apt install
        // plus a service start, and 60s is not a budget for that.
        match agent
            .post_long("/services/install/powerdns", forward_body, 900)
            .await
        {
            Ok(result) => {
                // Auto-save API URL and key to settings
                if let (Some(url), Some(key)) = (
                    result.get("api_url").and_then(|v| v.as_str()),
                    result.get("api_key").and_then(|v| v.as_str()),
                ) {
                    let _ = sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('pdns_api_url', $1, NOW()) ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = NOW()")
                        .bind(url)
                        .execute(&db)
                        .await;
                    let encrypted_key = crate::services::secrets_crypto::encrypt_credential(key, &jwt_secret)
                        .unwrap_or_else(|_| key.to_string());
                    let _ = sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('pdns_api_key', $1, NOW()) ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = NOW()")
                        .bind(&encrypted_key)
                        .execute(&db)
                        .await;
                }

                emit("install", "Installing PowerDNS", "done", None);
                emit("complete", "PowerDNS installed", "done", None);
                activity::log_activity(
                    &db, user_id, &email, "service.install",
                    Some("system"), Some("powerdns"), None, None,
                ).await;
                tracing::info!("Service installed: PowerDNS");
            }
            Err(e) => {
                emit("install", "Installing PowerDNS", "error", Some(format!("{e}")));
                emit("complete", "Install failed", "error", None);
                tracing::error!("Service install failed: PowerDNS: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(30)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&install_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "install_id": install_id,
        "message": "PowerDNS installation started",
    }))))
}

/// GET /api/system/disk-io — Proxy to agent's disk I/O stats (admin only).
pub async fn disk_io(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/system/disk-io")
        .await
        .map_err(|e| agent_error("Disk I/O", e))?;
    Ok(Json(data))
}

pub async fn install_fail2ban(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Fail2Ban", "/services/install/fail2ban", None).await
}

pub async fn install_redis(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Redis", "/services/install/redis", None).await
}

/// Install `sshpass` on the scoped server, for password-authenticated SFTP
/// backup destinations. `ServerScope` rather than a `server_id` in the body:
/// this runs a package transaction as root, so the target must be a server the
/// caller is proven to own, not one they can name. Issue #93.
pub async fn install_sshpass(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "sshpass", "/services/install/sshpass", None).await
}

pub async fn install_nodejs(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Node.js", "/services/install/nodejs", None).await
}

pub async fn install_composer(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Composer", "/services/install/composer", None).await
}

pub async fn install_waf(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "WAF (ModSecurity)", "/services/install/waf", None).await
}

pub async fn install_cloudflared(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Cloudflare Tunnel", "/services/install/cloudflared", None).await
}

// ── Service uninstallers (proxy to agent, async with SSE progress) ───────

pub async fn uninstall_php(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "PHP (uninstall)", "/services/uninstall/php", None).await
}

pub async fn uninstall_certbot(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Certbot (uninstall)", "/services/uninstall/certbot", None).await
}

pub async fn uninstall_ufw(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "UFW Firewall (uninstall)", "/services/uninstall/ufw", None).await
}

pub async fn uninstall_fail2ban(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Fail2Ban (uninstall)", "/services/uninstall/fail2ban", None).await
}

pub async fn uninstall_powerdns(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "PowerDNS (uninstall)", "/services/uninstall/powerdns", None).await
}

pub async fn uninstall_redis(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Redis (uninstall)", "/services/uninstall/redis", None).await
}

pub async fn uninstall_nodejs(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Node.js (uninstall)", "/services/uninstall/nodejs", None).await
}

pub async fn uninstall_composer(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Composer (uninstall)", "/services/uninstall/composer", None).await
}

pub async fn uninstall_waf(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "WAF (uninstall)", "/services/uninstall/waf", None).await
}

pub async fn uninstall_cloudflared(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Cloudflare Tunnel (uninstall)", "/services/uninstall/cloudflared", None).await
}

/// POST /api/traefik/install — Install Traefik reverse proxy.
pub async fn traefik_install(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let acme_email = body.get("acme_email").and_then(|v| v.as_str()).unwrap_or("admin@localhost");

    let result = agent
        .post("/traefik/install", Some(serde_json::json!({ "acme_email": acme_email })))
        .await
        .map_err(|e| agent_error("Traefik install", e))?;

    // Save reverse_proxy setting
    sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('reverse_proxy', 'traefik', NOW()) ON CONFLICT (key) DO UPDATE SET value = 'traefik', updated_at = NOW()")
        .execute(&state.db).await.ok();

    activity::log_activity(&state.db, claims.sub, &claims.email, "traefik.install", Some("system"), None, None, None).await;

    Ok(Json(result))
}

/// POST /api/traefik/uninstall — Remove Traefik.
pub async fn traefik_uninstall(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .post("/traefik/uninstall", None)
        .await
        .map_err(|e| agent_error("Traefik uninstall", e))?;

    // Revert to nginx
    sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('reverse_proxy', 'nginx', NOW()) ON CONFLICT (key) DO UPDATE SET value = 'nginx', updated_at = NOW()")
        .execute(&state.db).await.ok();

    activity::log_activity(&state.db, claims.sub, &claims.email, "traefik.uninstall", Some("system"), None, None, None).await;

    Ok(Json(result))
}

/// GET /api/traefik/status — Get Traefik status.
pub async fn traefik_status(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .get("/traefik/status")
        .await
        .map_err(|e| agent_error("Traefik status", e))?;

    Ok(Json(result))
}
