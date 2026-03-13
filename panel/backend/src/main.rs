mod auth;
mod config;
pub mod error;
mod models;
mod routes;
mod services;

use axum::{http::Method, Router};
use sqlx::postgres::PgPoolOptions;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
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
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env();

    // Connect to PostgreSQL with retry (DB container may not be ready yet)
    let mut retries = 0u32;
    let db = loop {
        match PgPoolOptions::new()
            .max_connections(config.db_max_connections)
            .connect(&config.database_url)
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

    // Build CORS with explicit origin whitelist (before config moves into Arc)
    let allowed_origins: Vec<axum::http::HeaderValue> = config
        .base_url
        .parse::<axum::http::HeaderValue>()
        .into_iter()
        .chain("https://panel.example.com".parse::<axum::http::HeaderValue>())
        .chain("https://dockpanel.dev".parse::<axum::http::HeaderValue>())
        .collect();

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed_origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([axum::http::header::CONTENT_TYPE])
        .allow_credentials(true);

    let config = Arc::new(config);
    let listen_addr = config.listen_addr.clone();

    let state = AppState {
        db,
        config,
        agent,
        login_attempts: Arc::new(Mutex::new(HashMap::new())),
    };

    // Spawn backup scheduler
    let scheduler_db = state.db.clone();
    let scheduler_agent = state.agent.clone();
    tokio::spawn(async move {
        services::backup_scheduler::run(scheduler_db, scheduler_agent).await;
    });

    // Spawn server health monitor
    let monitor_db = state.db.clone();
    tokio::spawn(async move {
        services::server_monitor::run(monitor_db).await;
    });

    // Spawn uptime monitor
    let uptime_db = state.db.clone();
    tokio::spawn(async move {
        services::uptime::run(uptime_db).await;
    });

    // Spawn security scanner (weekly)
    let scanner_db = state.db.clone();
    let scanner_agent = state.agent.clone();
    tokio::spawn(async move {
        services::security_scanner::run(scanner_db, scanner_agent).await;
    });

    // Spawn alert engine
    let alert_db = state.db.clone();
    let alert_agent = state.agent.clone();
    tokio::spawn(async move {
        services::alert_engine::run(alert_db, alert_agent).await;
    });

    // Spawn auto-healer
    let healer_db = state.db.clone();
    let healer_agent = state.agent.clone();
    tokio::spawn(async move {
        services::auto_healer::run(healer_db, healer_agent).await;
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
