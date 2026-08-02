#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod auth;
mod config;
pub mod error;
pub mod helpers;
mod models;
mod routes;
pub mod safe_cmd;
mod services;

use axum::{http::Method, Router};
use chrono;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use config::Config;
use services::agent::{AgentClient, AgentRegistry};

#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
    pub config: Arc<Config>,
    /// Legacy single-agent accessor (routes being migrated will use `agents` instead).
    pub agent: AgentClient,
    /// Multi-server agent registry: dispatches to local or remote agents by server_id.
    pub agents: AgentRegistry,
    pub login_attempts: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    /// Blacklisted JWT JTIs (for logout). Entries expire naturally after 2h.
    pub token_blacklist: Arc<RwLock<HashSet<String>>>,
    /// Rate limiter for 2FA verification attempts: user_id -> (count, window_start)
    pub twofa_attempts: Arc<Mutex<HashMap<uuid::Uuid, (u32, Instant)>>>,
    /// Rate limiter for deploy webhooks: site_id -> (failed_count, window_start)
    pub webhook_attempts: Arc<Mutex<HashMap<uuid::Uuid, (u32, Instant)>>>,
    /// Rate limiter for agent endpoints: server_id -> (count, window_start)
    pub agent_rate_limits: Arc<Mutex<HashMap<uuid::Uuid, (u32, Instant)>>>,
    /// Provisioning log channels: site_id -> (step history, broadcast sender)
    pub provision_logs: Arc<Mutex<HashMap<uuid::Uuid, (Vec<routes::sites::ProvisionStep>, tokio::sync::broadcast::Sender<routes::sites::ProvisionStep>, Instant)>>>,
    /// OAuth CSRF state tokens: state_string -> (provider_name, created_at)
    pub oauth_states: Arc<Mutex<HashMap<String, (String, Instant)>>>,
    /// Broadcast channel for real-time panel notification delivery (SSE)
    pub notif_tx: tokio::sync::broadcast::Sender<(uuid::Uuid, String)>,
    /// Cached `sessions_revoked_at` timestamp (epoch seconds).
    /// Auth middleware rejects tokens with `iat` before this value.
    /// Updated when admin calls revoke-all; avoids a DB query per request.
    pub sessions_revoked_at: Arc<RwLock<Option<i64>>>,
    /// Deploy ownership map: deploy_id -> user_id (for SSE log access control).
    pub deploy_owners: Arc<Mutex<HashMap<uuid::Uuid, uuid::Uuid>>>,
    /// WebAuthn/Passkey challenge store (in-memory, 5-minute TTL).
    pub passkey_challenges: routes::passkeys::ChallengeStore,
    /// Phase 4 W4: panel self-update orchestrator state. Read by
    /// `/api/update/status`; written by `start_panel_update`. In-process
    /// only — DB rows in `panel_snapshots` are the durable cross-restart
    /// signal (the api process dies mid-binary-swap).
    pub panel_update_state: services::panel_update::UpdateStateHandle,
}

/// Write the generated guidance artefacts and exit.
///
/// The point of the guidance layer's copy registry is that the manual is
/// *emitted* by the product rather than written alongside it, so this is the
/// literal mechanism: the shipped binary can print its own documentation. The
/// test `guidance_manual_is_current` runs the same functions and fails when the
/// committed files differ, which is what makes drift impossible rather than
/// merely discouraged.
///
/// Usage: `dockpanel-api --emit-guidance <repo-root>`
fn emit_guidance(root: &str) -> std::io::Result<()> {
    use services::prerequisites::copy;

    let manual_path = std::path::Path::new(root).join("docs/guides/prerequisites.md");
    let module_path = std::path::Path::new(root)
        .join("panel/frontend/src/content/guidance.generated.ts");

    if let Some(dir) = module_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&manual_path, copy::manual())?;
    std::fs::write(&module_path, copy::frontend_module())?;

    println!("wrote {}", manual_path.display());
    println!("wrote {}", module_path.display());
    Ok(())
}

#[tokio::main]
async fn main() {
    // Documentation generation runs before anything else is set up: it needs no
    // database, no config and no network, and a developer regenerating docs
    // should never risk touching a live system to do it.
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--emit-guidance") {
        let root = args.get(i + 1).map(String::as_str).unwrap_or(".");
        if let Err(e) = emit_guidance(root) {
            eprintln!("failed to emit guidance docs: {e}");
            std::process::exit(1);
        }
        return;
    }

    // Install rustls CryptoProvider before any TLS usage. Required by rustls 0.23+
    // when constructing a ClientConfig with a custom ServerCertVerifier (e.g., the
    // PinnedFingerprintVerifier used for remote-agent TLS pinning). Without this,
    // the first outbound pinned TLS handshake panics in rustls::crypto::mod.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install rustls aws_lc_rs CryptoProvider");

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let log_format = std::env::var("LOG_FORMAT").unwrap_or_default();
    if log_format == "json" {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .init();
    }

    let config = Config::from_env();

    // Connect to PostgreSQL with retry (DB container may not be ready yet)
    let connect_opts = PgConnectOptions::from_str(&config.database_url)
        .expect("Invalid DATABASE_URL");

    const DB_MAX_RETRIES: u32 = 5;
    const DB_RETRY_DELAY: Duration = Duration::from_secs(3);

    let mut retries = 0u32;
    let db = loop {
        match PgPoolOptions::new()
            .max_connections(config.db_max_connections)
            .min_connections(2)
            .acquire_timeout(Duration::from_secs(5))
            // Note: slow query logging (log_min_duration_statement) should be configured
            // in postgresql.conf, not per-connection. Set to 1000ms for production.
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout = '30000'")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(connect_opts.clone())
            .await
        {
            Ok(pool) => break pool,
            Err(e) => {
                retries += 1;
                if retries >= DB_MAX_RETRIES {
                    tracing::error!(
                        "Failed to connect to database after {retries} attempts: {e}"
                    );
                    return;
                }
                tracing::warn!(
                    "Database not ready (attempt {retries}/{DB_MAX_RETRIES}): {e}, retrying in {}s...",
                    DB_RETRY_DELAY.as_secs()
                );
                tokio::time::sleep(DB_RETRY_DELAY).await;
            }
        }
    };

    // Run migrations.
    //
    // `ignore_missing(true)` is required because this panel supports ROLLBACK
    // (W4: update.sh's .bak restore and /api/update/rollback). After an update
    // applies migration N+1 and is then rolled back, the older binary sees an
    // applied migration it has no file for; sqlx's default strict validation
    // rejects that with `VersionMissing(...)`, and since the call site panics,
    // the restored api exits 101 and crash-loops under Restart=always until it
    // hits StartLimitBurst and lands in `failed` — a permanent 502 with no
    // operator-facing explanation. Verified on a lab box: injecting one unknown
    // `_sqlx_migrations` row is enough to brick startup.
    //
    // Migrations here are additive, so an older binary running against a newer
    // schema is safe; it simply ignores columns it does not know about. Missing
    // migrations are still applied — only *unknown extra* ones are tolerated.
    sqlx::migrate!("./migrations")
        .set_ignore_missing(true)
        .run(&db)
        .await
        .expect("Failed to run database migrations");

    tracing::info!("Database connected and migrations applied");

    // Create agent client (local) and agent registry (multi-server)
    let agent = AgentClient::new(config.agent_socket.clone(), config.agent_token.clone());
    let agents = AgentRegistry::new(agent.clone(), db.clone());

    // Ensure local server exists in DB and register its ID in the registry
    let local_server_id = services::agent::ensure_local_server(&db, &config.agent_token).await;
    if !local_server_id.is_nil() {
        agents.set_local_server_id(local_server_id).await;
        tracing::info!("Local server ID: {local_server_id}");
    }

    // Build CORS policy.
    //
    // The panel frontend is always served by nginx on the same origin as the API
    // (nginx proxies /api/* to the backend). So frontend→API calls are same-origin
    // and don't need CORS at all.
    //
    // CORS only applies to cross-origin requests (other websites calling the API).
    // When CORS_ORIGINS is not configured, we deny all cross-origin requests.
    // When configured (e.g. for development or external integrations), we whitelist.
    let cors = if config.cors_origins.is_empty() {
        // No CORS origins configured — deny all cross-origin requests.
        // Same-origin requests (the panel UI) are unaffected by CORS.
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(Vec::<axum::http::HeaderValue>::new()))
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([
                axum::http::header::CONTENT_TYPE,
                axum::http::header::AUTHORIZATION,
                axum::http::header::ACCEPT,
                axum::http::HeaderName::from_static("x-server-id"),
            ])
    } else {
        let allowed_origins: Vec<axum::http::HeaderValue> = config
            .cors_origins
            .iter()
            .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(allowed_origins))
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([
                axum::http::header::CONTENT_TYPE,
                axum::http::header::AUTHORIZATION,
                axum::http::header::ACCEPT,
                axum::http::HeaderName::from_static("x-server-id"),
            ])
            .allow_credentials(true)
    };

    let config = Arc::new(config);
    let listen_addr = config.listen_addr.clone();

    // Broadcast channel for real-time notification delivery via SSE
    let (notif_tx, _) = tokio::sync::broadcast::channel::<(uuid::Uuid, String)>(256);
    // Register in the global OnceLock so notify_panel() can broadcast without AppState
    services::notifications::init_notif_broadcast(notif_tx.clone());

    // GAP 66: Load persisted token blacklist from DB (survives restart)
    let token_blacklist = {
        let blacklisted: Vec<(String,)> = sqlx::query_as(
            "SELECT jti FROM token_blacklist WHERE expires_at > NOW()"
        ).fetch_all(&db).await.unwrap_or_default();
        let mut bl = HashSet::new();
        for (jti,) in blacklisted {
            bl.insert(jti);
        }
        if !bl.is_empty() {
            tracing::info!("Loaded {} blacklisted tokens from DB", bl.len());
        }
        Arc::new(RwLock::new(bl))
    };

    // Load sessions_revoked_at from settings table (survives restart)
    let sessions_revoked_at = {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT value FROM settings WHERE key = 'sessions_revoked_at'"
        ).fetch_optional(&db).await.ok().flatten();
        let ts = row.and_then(|r| {
            chrono::DateTime::parse_from_rfc3339(&r.0).ok().map(|dt| dt.timestamp())
        });
        if ts.is_some() {
            tracing::info!("Loaded sessions_revoked_at from DB");
        }
        Arc::new(RwLock::new(ts))
    };

    let state = AppState {
        db,
        config,
        agent,
        agents,
        login_attempts: Arc::new(Mutex::new(HashMap::new())),
        token_blacklist,
        twofa_attempts: Arc::new(Mutex::new(HashMap::new())),
        webhook_attempts: Arc::new(Mutex::new(HashMap::new())),
        agent_rate_limits: Arc::new(Mutex::new(HashMap::new())),
        provision_logs: Arc::new(Mutex::new(HashMap::new())),
        oauth_states: Arc::new(Mutex::new(HashMap::new())),
        notif_tx,
        sessions_revoked_at,
        deploy_owners: Arc::new(Mutex::new(HashMap::new())),
        passkey_challenges: routes::passkeys::new_challenge_store(),
        panel_update_state: services::panel_update::new_state_handle(),
    };

    // Phase 4 W4: close out any in-flight panel-update rows from a previous
    // process lifetime. If a snapshot row exists with to_version IS NULL and
    // we just booted, the prior api crashed/was killed mid-update; write
    // to_version = CARGO_PKG_VERSION so the UI shows succeeded vs rolled-back.
    services::panel_update::finalize_pending_on_startup(&state.db).await;

    // Same argument, one table over: migration analysis and import both run in a
    // spawned task, so a restart takes them with no chance to write a verdict.
    // On boot nothing is running, so any row still claiming to be is stale.
    routes::migration::finalize_analyzing_on_startup(&state.db).await;

    // Shutdown broadcast channel — all background services listen for this signal
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    // Supervised background task spawner: monitors JoinHandle, auto-restarts on panic
    // with exponential backoff, and respects shutdown signal.
    fn spawn_supervised<F, Fut>(
        name: &'static str,
        shutdown_tx: &tokio::sync::broadcast::Sender<()>,
        f: F,
    ) where
        F: Fn(tokio::sync::broadcast::Receiver<()>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            let mut delay = Duration::from_secs(1);
            const MAX_DELAY: Duration = Duration::from_secs(300);
            // If the task runs longer than this without panicking, reset backoff
            const HEALTHY_THRESHOLD: Duration = Duration::from_secs(60);

            loop {
                let mut shutdown_rx = shutdown_tx.subscribe();
                let started = Instant::now();
                let handle = tokio::spawn(f(shutdown_tx.subscribe()));

                tokio::select! {
                    result = handle => {
                        match result {
                            Ok(()) => {
                                tracing::warn!("Background task '{name}' exited");
                            }
                            Err(e) => {
                                tracing::error!("Background task '{name}' panicked: {e}");
                            }
                        }

                        // Reset backoff if the task ran healthily for a while
                        if started.elapsed() >= HEALTHY_THRESHOLD {
                            delay = Duration::from_secs(1);
                        }

                        // Check if shutdown was requested before restarting
                        if shutdown_tx.receiver_count() == 0 {
                            break;
                        }

                        tracing::info!("Restarting '{name}' in {}s (backoff)", delay.as_secs());
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(MAX_DELAY);
                    }
                    _ = shutdown_rx.recv() => {
                        tracing::info!("Supervisor for '{name}' received shutdown signal");
                        break;
                    }
                }
            }
        });
    }

    // Status-page subscriber fan-out worker. Started here rather than from a
    // service's run loop because BOTH producers feed it — the uptime monitor and
    // the incidents HTTP handlers — and a request can arrive before any
    // background task has had a chance to run. Idempotent.
    services::status_notices::start_worker(state.db.clone());

    // Spawn supervised background tasks
    // Fleet-wide schedule query, so it gets the registry: a site is backed up on the
    // server that holds its files. See `run_scheduled_backup`.
    let (s_db, s_agents) = (state.db.clone(), state.agents.clone());
    // The scheduler decrypts database credentials to include them in site
    // backups, so it needs the same secret the restore path uses.
    let s_jwt_bs = state.config.jwt_secret.clone();
    spawn_supervised("backup_scheduler", &shutdown_tx, move |rx| services::backup_scheduler::run(s_db.clone(), s_agents.clone(), s_jwt_bs.clone(), rx));

    let s_db = state.db.clone();
    spawn_supervised("server_monitor", &shutdown_tx, move |rx| services::server_monitor::run(s_db.clone(), rx));

    let s_db = state.db.clone();
    spawn_supervised("uptime_monitor", &shutdown_tx, move |rx| services::uptime::run(s_db.clone(), rx));

    // Both scanners take a MACHINE as their subject rather than a row, so each
    // sweeps the whole fleet through the registry. See `online_fleet`.
    let (s_db, s_agents) = (state.db.clone(), state.agents.clone());
    spawn_supervised("security_scanner", &shutdown_tx, move |rx| services::security_scanner::run(s_db.clone(), s_agents.clone(), rx));

    let (s_db, s_agents) = (state.db.clone(), state.agents.clone());
    spawn_supervised("image_scanner", &shutdown_tx, move |rx| services::image_scanner::run(s_db.clone(), s_agents.clone(), rx));

    // Its GPU, service-health and container checks each ask ONE machine what it
    // has, so each runs per online server against that server's own agent.
    // See `online_fleet`.
    let (s_db, s_agents) = (state.db.clone(), state.agents.clone());
    spawn_supervised("alert_engine", &shutdown_tx, move |rx| services::alert_engine::run(s_db.clone(), s_agents.clone(), rx));

    // The healer gets the REGISTRY as well as the legacy local client: its disk
    // heal acts on whichever server's alert is firing, which is not necessarily
    // this one. See `auto_clean_disk`.
    let (s_db, s_agent, s_agents) = (state.db.clone(), state.agent.clone(), state.agents.clone());
    spawn_supervised("auto_healer", &shutdown_tx, move |rx| services::auto_healer::run(s_db.clone(), s_agent.clone(), s_agents.clone(), rx));

    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("metrics_collector", &shutdown_tx, move |rx| services::metrics_collector::run(s_db.clone(), s_agent.clone(), rx));

    // The deploy scheduler queries EVERY server's cron deploys, so it gets the
    // registry: each row is deployed on the server that owns it. See
    // `trigger_deploy_task`.
    let (s_db, s_agents) = (state.db.clone(), state.agents.clone());
    spawn_supervised("deploy_scheduler", &shutdown_tx, move |rx| services::deploy_scheduler::run(s_db.clone(), s_agents.clone(), rx));

    // Previews are torn down on the server their git deploy lives on, resolved
    // through the JOIN the sweep already performs.
    let (s_db, s_agents) = (state.db.clone(), state.agents.clone());
    spawn_supervised("preview_cleanup", &shutdown_tx, move |rx| services::preview_cleanup::run(s_db.clone(), s_agents.clone(), rx));

    // Every verifier query is fleet-wide, so it gets the registry: an archive is read
    // on the host that wrote it. See `verify_one`.
    let (s_db, s_agents) = (state.db.clone(), state.agents.clone());
    spawn_supervised("backup_verifier", &shutdown_tx, move |rx| services::backup_verifier::run(s_db.clone(), s_agents.clone(), rx));

    // Each leg resolves its own host: sites and databases per row, volumes per policy.
    // See `execute_policy`.
    let (s_db, s_agents, s_jwt) = (state.db.clone(), state.agents.clone(), state.config.jwt_secret.clone());
    spawn_supervised("backup_policy_executor", &shutdown_tx, move |rx| services::backup_policy_executor::run(s_db.clone(), s_agents.clone(), s_jwt.clone(), rx));

    // A drill restores a backup on the server that owns it — running one elsewhere
    // certifies DR for a machine it never tested. See `dispatch_policy_drills`.
    let (s_db, s_agents) = (state.db.clone(), state.agents.clone());
    spawn_supervised("drill_scheduler", &shutdown_tx, move |rx| services::drill_scheduler::run(s_db.clone(), s_agents.clone(), rx));

    // Local BY INTENT, not by omission: it diagnoses the panel host itself. It takes
    // the registry so that intent is stated in the type and calls `agents.local()`.
    let (s_db, s_agents) = (state.db.clone(), state.agents.clone());
    spawn_supervised("telemetry_collector", &shutdown_tx, move |rx| services::telemetry_collector::run(s_db.clone(), s_agents.clone(), rx));

    // Periodic cleanup of token blacklist and rate limiters (every 15 minutes)
    let cleanup_blacklist = state.token_blacklist.clone();
    let cleanup_bl_db = state.db.clone();
    let cleanup_login = state.login_attempts.clone();
    let cleanup_twofa = state.twofa_attempts.clone();
    let cleanup_webhook = state.webhook_attempts.clone();
    let cleanup_agent_rl = state.agent_rate_limits.clone();
    let cleanup_provision = state.provision_logs.clone();
    let cleanup_deploy_owners = state.deploy_owners.clone();
    let cleanup_oauth = state.oauth_states.clone();
    spawn_supervised("cleanup", &shutdown_tx, move |mut shutdown_rx| {
        let blacklist = cleanup_blacklist.clone();
        let bl_db = cleanup_bl_db.clone();
        let login = cleanup_login.clone();
        let twofa = cleanup_twofa.clone();
        let webhook = cleanup_webhook.clone();
        let agent_rl = cleanup_agent_rl.clone();
        let provision = cleanup_provision.clone();
        let deploy_owners = cleanup_deploy_owners.clone();
        let oauth = cleanup_oauth.clone();
        async move {
        let mut interval = tokio::time::interval(Duration::from_secs(900));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown_rx.recv() => {
                    tracing::info!("Cleanup task shutting down gracefully");
                    break;
                }
            }
            // Clean token blacklist: if over 10000 entries, purge expired from DB and reload
            let bl_count = blacklist.read().await.len();
            if bl_count > 10000 {
                let _ = sqlx::query("DELETE FROM token_blacklist WHERE expires_at <= NOW()")
                    .execute(&bl_db).await;
                let active: Vec<(String,)> = sqlx::query_as(
                    "SELECT jti FROM token_blacklist WHERE expires_at > NOW()"
                ).fetch_all(&bl_db).await.unwrap_or_default();
                let mut bl = blacklist.write().await;
                bl.clear();
                for (jti,) in &active {
                    bl.insert(jti.clone());
                }
                tracing::info!("Token blacklist cleaned: {} -> {} entries (reloaded from DB)", bl_count, bl.len());
            }
            // Clean expired rate limit entries
            let now = Instant::now();
            let window_15m = Duration::from_secs(900);
            let window_5m = Duration::from_secs(300);
            if let Ok(mut map) = login.lock() {
                map.retain(|_, attempts| {
                    attempts.retain(|t| now.duration_since(*t) < window_15m);
                    !attempts.is_empty()
                });
            }
            if let Ok(mut map) = twofa.lock() {
                map.retain(|_, (_, start)| now.duration_since(*start) < window_5m);
            }
            if let Ok(mut map) = webhook.lock() {
                map.retain(|_, (_, start)| now.duration_since(*start) < window_5m);
            }
            if let Ok(mut map) = agent_rl.lock() {
                map.retain(|_, (_, start)| now.duration_since(*start) < Duration::from_secs(60));
            }
            // Clean stale provisioning logs.
            //
            // The window has to be longer than the LONGEST job that writes into
            // this map, because eviction does not stop the job — it silently
            // detaches it. `emit` looks the id up and finds nothing, so every
            // remaining step, including the terminal one, goes nowhere; the SSE
            // stream ends without a verdict and the page watching it waits for a
            // "complete" that can no longer be sent.
            //
            // Five minutes had been shorter than several of them for a while: a
            // migration import budgets 300s per site and 600s per database, and
            // a PHP version install can add a third-party repository before it
            // unpacks fifteen packages. An hour costs a few hundred bytes per
            // finished job and covers all of them; the terminal-step removals in
            // each feature are what actually reclaim the common case.
            //
            // The owner prune below runs on every tick, not only on ticks where
            // this TTL evicted something. Each feature removes its own log 30-60s
            // after its job ends, so the one-hour sweep usually finds nothing to
            // drop — which, while the prune was conditional on it, meant the owner
            // map was almost never pruned and grew for the life of the process.
            // That was survivable when three call sites registered an owner. Every
            // provisioning log has one now, so it is not.
            //
            // Lock order is logs then owners, matching `register_provision_log`
            // and `open_provision_log`, so a registration cannot interleave and
            // leave a live log whose owner has just been swept out from under it.
            // Poison-tolerant, like every other accessor of these two maps. An
            // `if let Ok(...)` here would make a single panic under either lock
            // permanently disable the only pruner they have — writers would keep
            // inserting through `into_inner()` while nothing ever removed, which
            // is the unbounded growth this block exists to prevent.
            {
                let mut map = provision.lock().unwrap_or_else(|e| e.into_inner());
                map.retain(|_, (_, _, created)| now.duration_since(*created) < Duration::from_secs(3600));
                let mut owners = deploy_owners.lock().unwrap_or_else(|e| e.into_inner());
                owners.retain(|id, _| map.contains_key(id));
            }
            // Clean expired OAuth CSRF states (older than 10 minutes)
            if let Ok(mut map) = oauth.lock() {
                map.retain(|_, (_, created)| now.duration_since(*created) < Duration::from_secs(600));
            }
        }
    }});

    let shutdown_db = state.db.clone();

    let app = Router::new()
        .merge(routes::router())
        .layer(cors)
        .layer(TimeoutLayer::with_status_code(axum::http::StatusCode::GATEWAY_TIMEOUT, Duration::from_secs(300)))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .expect("Failed to bind TCP listener");

    tracing::info!(
        "DockPanel API v{} listening on {listen_addr}",
        env!("CARGO_PKG_VERSION")
    );

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!("API server error: {e}");
    }

    // Signal all background services to stop
    tracing::info!("Sending shutdown signal to background services...");
    let _ = shutdown_tx.send(());
    // Give services a moment to finish their current work
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Drain the connection pool so active queries finish before process exits
    shutdown_db.close().await;
    tracing::info!("Database connection pool closed");

    tracing::info!("DockPanel API shut down gracefully");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("Received Ctrl+C, shutting down..."),
        _ = terminate => tracing::info!("Received SIGTERM, shutting down..."),
    }
}
