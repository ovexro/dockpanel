mod auth;
mod config;
pub mod error;
pub mod helpers;
mod models;
mod routes;
mod services;

use axum::{http::Method, Router};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tower_http::cors::{AllowOrigin, CorsLayer};
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
}

#[tokio::main]
async fn main() {
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

    // Run migrations
    sqlx::migrate!("./migrations")
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

    // Build CORS with configurable origin whitelist (CORS_ORIGINS env var or defaults)
    let allowed_origins: Vec<axum::http::HeaderValue> = config
        .cors_origins
        .iter()
        .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
        .collect();

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed_origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::header::ACCEPT,
            axum::http::HeaderName::from_static("x-server-id"),
        ])
        .allow_credentials(true);

    let config = Arc::new(config);
    let listen_addr = config.listen_addr.clone();

    let state = AppState {
        db,
        config,
        agent,
        agents,
        login_attempts: Arc::new(Mutex::new(HashMap::new())),
        token_blacklist: Arc::new(RwLock::new(HashSet::new())),
        twofa_attempts: Arc::new(Mutex::new(HashMap::new())),
        webhook_attempts: Arc::new(Mutex::new(HashMap::new())),
        agent_rate_limits: Arc::new(Mutex::new(HashMap::new())),
        provision_logs: Arc::new(Mutex::new(HashMap::new())),
        oauth_states: Arc::new(Mutex::new(HashMap::new())),
    };

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

    // Spawn supervised background tasks
    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("backup_scheduler", &shutdown_tx, move |rx| services::backup_scheduler::run(s_db.clone(), s_agent.clone(), rx));

    let s_db = state.db.clone();
    spawn_supervised("server_monitor", &shutdown_tx, move |rx| services::server_monitor::run(s_db.clone(), rx));

    let s_db = state.db.clone();
    spawn_supervised("uptime_monitor", &shutdown_tx, move |rx| services::uptime::run(s_db.clone(), rx));

    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("security_scanner", &shutdown_tx, move |rx| services::security_scanner::run(s_db.clone(), s_agent.clone(), rx));

    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("alert_engine", &shutdown_tx, move |rx| services::alert_engine::run(s_db.clone(), s_agent.clone(), rx));

    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("auto_healer", &shutdown_tx, move |rx| services::auto_healer::run(s_db.clone(), s_agent.clone(), rx));

    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("metrics_collector", &shutdown_tx, move |rx| services::metrics_collector::run(s_db.clone(), s_agent.clone(), rx));

    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("deploy_scheduler", &shutdown_tx, move |rx| services::deploy_scheduler::run(s_db.clone(), s_agent.clone(), rx));

    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("preview_cleanup", &shutdown_tx, move |rx| services::preview_cleanup::run(s_db.clone(), s_agent.clone(), rx));

    // Periodic cleanup of token blacklist and rate limiters (every 15 minutes)
    let cleanup_blacklist = state.token_blacklist.clone();
    let cleanup_login = state.login_attempts.clone();
    let cleanup_twofa = state.twofa_attempts.clone();
    let cleanup_webhook = state.webhook_attempts.clone();
    let cleanup_agent_rl = state.agent_rate_limits.clone();
    let cleanup_provision = state.provision_logs.clone();
    let cleanup_oauth = state.oauth_states.clone();
    let mut cleanup_shutdown_rx = shutdown_tx.subscribe();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(900));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = cleanup_shutdown_rx.recv() => {
                    tracing::info!("Cleanup task shutting down gracefully");
                    break;
                }
            }
            // Clear all blacklisted tokens (JWT expiry is 2h, cleanup runs every 15m)
            let removed = {
                let mut bl = cleanup_blacklist.write().await;
                let count = bl.len();
                if count > 10000 {
                    bl.clear();
                    count
                } else {
                    0
                }
            };
            if removed > 0 {
                tracing::info!("Cleaned {removed} entries from token blacklist");
            }
            // Clean expired rate limit entries
            let now = Instant::now();
            let window_15m = Duration::from_secs(900);
            let window_5m = Duration::from_secs(300);
            if let Ok(mut map) = cleanup_login.lock() {
                map.retain(|_, attempts| {
                    attempts.retain(|t| now.duration_since(*t) < window_15m);
                    !attempts.is_empty()
                });
            }
            if let Ok(mut map) = cleanup_twofa.lock() {
                map.retain(|_, (_, start)| now.duration_since(*start) < window_5m);
            }
            if let Ok(mut map) = cleanup_webhook.lock() {
                map.retain(|_, (_, start)| now.duration_since(*start) < window_5m);
            }
            if let Ok(mut map) = cleanup_agent_rl.lock() {
                map.retain(|_, (_, start)| now.duration_since(*start) < Duration::from_secs(60));
            }
            // Clean stale provisioning logs (older than 5 minutes)
            if let Ok(mut map) = cleanup_provision.lock() {
                map.retain(|_, (_, _, created)| now.duration_since(*created) < Duration::from_secs(300));
            }
            // Clean expired OAuth CSRF states (older than 10 minutes)
            if let Ok(mut map) = cleanup_oauth.lock() {
                map.retain(|_, (_, created)| now.duration_since(*created) < Duration::from_secs(600));
            }
        }
    });

    let shutdown_db = state.db.clone();

    let app = Router::new()
        .merge(routes::router())
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .expect("Failed to bind TCP listener");

    tracing::info!(
        "DockPanel API v{} listening on {listen_addr}",
        env!("CARGO_PKG_VERSION")
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();

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
