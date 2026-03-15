mod routes;
mod services;

use axum::{middleware, Router};
use bollard::Docker;
use std::path::Path;
use std::sync::Arc;
use sysinfo::System;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

const SOCKET_PATH: &str = "/var/run/dockpanel/agent.sock";
const CONFIG_DIR: &str = "/etc/dockpanel";

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

    // Ensure directories exist
    std::fs::create_dir_all("/var/run/dockpanel").ok();
    std::fs::create_dir_all(CONFIG_DIR).ok();
    std::fs::create_dir_all("/etc/dockpanel/ssl").ok();
    std::fs::create_dir_all("/var/backups/dockpanel").ok();
    std::fs::create_dir_all("/var/www/acme/.well-known/acme-challenge").ok();

    // Load or generate auth token
    let token_path = format!("{CONFIG_DIR}/agent.token");
    let token = match std::fs::read_to_string(&token_path) {
        Ok(t) => t.trim().to_string(),
        Err(_) => {
            let t = uuid::Uuid::new_v4().to_string();
            std::fs::write(&token_path, &t).expect("Failed to write agent token");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    &token_path,
                    std::fs::Permissions::from_mode(0o600),
                )
                .ok();
            }
            tracing::info!("Generated new agent token (saved to {token_path})");
            t
        }
    };

    // Initialize Tera templates
    let templates = services::nginx::init_templates();

    // Initialize cached System instance (refreshed per request instead of rebuilt)
    let mut sys = System::new_all();
    sys.refresh_all();

    // Initialize shared Docker client
    let docker = Docker::connect_with_local_defaults()
        .expect("Failed to connect to Docker daemon");

    // Build shared state
    let state = routes::AppState {
        token,
        templates,
        system: Arc::new(Mutex::new(sys)),
        docker,
    };

    // Build router with auth middleware
    // Terminal WS validates its own token via query param, so it's outside the middleware.
    let app = Router::new()
        .merge(routes::health::router())
        .merge(routes::system::router())
        .merge(routes::nginx::router())
        .merge(routes::ssl::router())
        .merge(routes::database::router())
        .merge(routes::files::router())
        .merge(routes::backups::router())
        .merge(routes::logs::router())
        .merge(routes::docker_apps::router())
        .merge(routes::security::router())
        .merge(routes::crons::router())
        .merge(routes::php::router())
        .merge(routes::remote_backup::router())
        .merge(routes::deploy::router())
        .merge(routes::smtp::router())
        .merge(routes::wordpress::router())
        .merge(routes::staging::router())
        .merge(routes::services::router())
        .merge(routes::iac::router())
        .merge(routes::updates::router())
        .merge(routes::diagnostics::router())
        .merge(routes::mail::router())
        .merge(routes::service_installer::router())
        .merge(routes::server_utils::router())
        .layer(middleware::from_fn_with_state(state.clone(), routes::auth_middleware))
        .layer(middleware::from_fn(routes::audit_middleware))
        .merge(routes::terminal::router())
        .merge(routes::logs::stream_router())
        .with_state(state);

    // Remove stale socket
    if Path::new(SOCKET_PATH).exists() {
        std::fs::remove_file(SOCKET_PATH).ok();
    }

    let listener = UnixListener::bind(SOCKET_PATH).expect("Failed to bind Unix socket");

    // Set socket permissions so Docker containers can access it
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(SOCKET_PATH, std::fs::Permissions::from_mode(0o660)).ok();
    }

    // Start phone-home if configured (remote agent mode)
    let remote_mode = if let Some(ph_config) = services::phone_home::PhoneHomeConfig::from_env() {
        tokio::spawn(services::phone_home::run(ph_config));
        true
    } else {
        false
    };

    // In remote mode, also start a TCP listener on localhost for command forwarding
    if remote_mode {
        let tcp_app = app.clone();
        tokio::spawn(async move {
            let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:9090")
                .await
                .expect("Failed to bind TCP listener for remote mode");
            tracing::info!("Agent TCP listener on 127.0.0.1:9090 (remote command forwarding)");
            axum::serve(tcp_listener, tcp_app).await.unwrap();
        });
    }

    tracing::info!(
        "DockPanel Agent v{} listening on {SOCKET_PATH}",
        env!("CARGO_PKG_VERSION")
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();

    tracing::info!("DockPanel Agent shut down gracefully");
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
