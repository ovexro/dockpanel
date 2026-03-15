mod auth;
mod config;
pub mod error;
mod models;
mod routes;
mod services;

use axum::{http::Method, Router};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::RwLock;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use config::Config;
use services::agent::AgentClient;

#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
    pub config: Arc<Config>,
    pub agent: AgentClient,
    pub login_attempts: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    /// Blacklisted JWT JTIs (for logout). Entries expire naturally after 2h.
    pub token_blacklist: Arc<RwLock<HashSet<String>>>,
    /// Rate limiter for 2FA verification attempts: user_id -> (count, window_start)
    pub twofa_attempts: Arc<Mutex<HashMap<uuid::Uuid, (u32, Instant)>>>,
    /// Rate limiter for deploy webhooks: site_id -> (failed_count, window_start)
    pub webhook_attempts: Arc<Mutex<HashMap<uuid::Uuid, (u32, Instant)>>>,
    /// Rate limiter for agent endpoints: server_id -> (count, window_start)
    pub agent_rate_limits: Arc<Mutex<HashMap<uuid::Uuid, (u32, Instant)>>>,
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

    let mut retries = 0u32;
    let db = loop {
        match PgPoolOptions::new()
            .max_connections(config.db_max_connections)
            .min_connections(2)
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
                if retries >= 30 {
                    tracing::error!(
                        "Failed to connect to database after {retries} attempts: {e}"
                    );
                    std::process::exit(1);
                }
                tracing::warn!("Database not ready ({retries}/30): {e}, retrying in 2s...");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    };

    // Run migrations
    sqlx::migrate!("./migrations")
        .run(&db)
        .await
        .expect("Failed to run database migrations");

    tracing::info!("Database connected and migrations applied");

    // Create agent client
    let agent = AgentClient::new(config.agent_socket.clone(), config.agent_token.clone());

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
        ])
        .allow_credentials(true);

    let config = Arc::new(config);
    let listen_addr = config.listen_addr.clone();

    let state = AppState {
        db,
        config,
        agent,
        login_attempts: Arc::new(Mutex::new(HashMap::new())),
        token_blacklist: Arc::new(RwLock::new(HashSet::new())),
        twofa_attempts: Arc::new(Mutex::new(HashMap::new())),
        webhook_attempts: Arc::new(Mutex::new(HashMap::new())),
        agent_rate_limits: Arc::new(Mutex::new(HashMap::new())),
    };

    // Supervised background task spawner: monitors JoinHandle, auto-restarts on panic
    fn spawn_supervised<F, Fut>(name: &'static str, f: F)
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(async move {
            loop {
                let handle = tokio::spawn(f());
                match handle.await {
                    Ok(()) => {
                        tracing::warn!("Background task '{name}' exited, restarting in 10s");
                    }
                    Err(e) => {
                        tracing::error!("Background task '{name}' panicked: {e}, restarting in 10s");
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        });
    }

    // Spawn supervised background tasks
    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("backup_scheduler", move || services::backup_scheduler::run(s_db.clone(), s_agent.clone()));

    let s_db = state.db.clone();
    spawn_supervised("server_monitor", move || services::server_monitor::run(s_db.clone()));

    let s_db = state.db.clone();
    spawn_supervised("uptime_monitor", move || services::uptime::run(s_db.clone()));

    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("security_scanner", move || services::security_scanner::run(s_db.clone(), s_agent.clone()));

    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("alert_engine", move || services::alert_engine::run(s_db.clone(), s_agent.clone()));

    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("auto_healer", move || services::auto_healer::run(s_db.clone(), s_agent.clone()));

    let (s_db, s_agent) = (state.db.clone(), state.agent.clone());
    spawn_supervised("metrics_collector", move || services::metrics_collector::run(s_db.clone(), s_agent.clone()));

    // Periodic cleanup of token blacklist and rate limiters (every 15 minutes)
    let cleanup_blacklist = state.token_blacklist.clone();
    let cleanup_login = state.login_attempts.clone();
    let cleanup_twofa = state.twofa_attempts.clone();
    let cleanup_webhook = state.webhook_attempts.clone();
    let cleanup_agent_rl = state.agent_rate_limits.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(900));
        loop {
            interval.tick().await;
            // Clear all blacklisted tokens (JWT expiry is 2h, cleanup runs every 15m)
            let removed = {
                let mut bl = cleanup_blacklist.write().await;
                let count = bl.len();
                if count > 1000 {
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
            let window_15m = std::time::Duration::from_secs(900);
            let window_5m = std::time::Duration::from_secs(300);
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
                map.retain(|_, (_, start)| now.duration_since(*start) < std::time::Duration::from_secs(60));
            }
        }
    });

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
