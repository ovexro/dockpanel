#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod routes;
pub mod safe_cmd;
mod services;
mod tls;

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
    // Install rustls CryptoProvider before any TLS usage
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

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
    std::fs::create_dir_all("/var/backups/dockpanel/databases").ok();
    std::fs::create_dir_all("/var/backups/dockpanel/volumes").ok();
    std::fs::create_dir_all("/var/www/acme/.well-known/acme-challenge").ok();

    // Load auth token: prefer AGENT_TOKEN env var, then file, then generate new
    let token_path = format!("{CONFIG_DIR}/agent.token");
    let token = if let Ok(env_token) = std::env::var("AGENT_TOKEN") {
        let env_token = env_token.trim().to_string();
        if !env_token.is_empty() {
            // Sync env token to file so both sources stay consistent
            std::fs::write(&token_path, &env_token).ok();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).ok();
            }
            tracing::info!("Using agent token from AGENT_TOKEN env var");
            env_token
        } else {
            // Empty env var — fall through to file
            match std::fs::read_to_string(&token_path) {
                Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
                _ => {
                    let t = uuid::Uuid::new_v4().to_string();
                    std::fs::write(&token_path, &t).expect("Failed to write agent token");
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).ok();
                    }
                    tracing::info!("Generated new agent token (saved to {token_path})");
                    t
                }
            }
        }
    } else {
        // No env var — use file or generate
        match std::fs::read_to_string(&token_path) {
            Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => {
                let t = uuid::Uuid::new_v4().to_string();
                std::fs::write(&token_path, &t).expect("Failed to write agent token");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).ok();
                }
                tracing::info!("Generated new agent token (saved to {token_path})");
                t
            }
        }
    };

    // Ensure token file permissions are restrictive
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&token_path) {
            let perms = meta.permissions();
            if perms.mode() & 0o777 != 0o600 {
                std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).ok();
            }
        }
    }

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
        token: Arc::new(tokio::sync::RwLock::new(token)),
        previous_token: Arc::new(tokio::sync::RwLock::new(None)),
        templates,
        system: Arc::new(Mutex::new(sys)),
        docker,
        network_snapshot: Arc::new(Mutex::new(None)),
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
        .merge(routes::image_scan::router())
        .merge(routes::sbom::router())
        .merge(routes::security::router())
        .merge(routes::crons::router())
        .merge(routes::php::router())
        .merge(routes::remote_backup::router())
        .merge(routes::database_backup::router())
        .merge(routes::volume_backup::router())
        .merge(routes::backup_verify::router())
        .merge(routes::deploy::router())
        .merge(routes::git_build::router())
        .merge(routes::smtp::router())
        .merge(routes::wordpress::router())
        .merge(routes::cms::router())
        .merge(routes::staging::router())
        .merge(routes::services::router())
        .merge(routes::iac::router())
        .merge(routes::updates::router())
        .merge(routes::diagnostics::router())
        .merge(routes::mail::router())
        .merge(routes::migration::router())
        .merge(routes::service_installer::router())
        .merge(routes::server_utils::router())
        .merge(routes::traefik::router())
        .merge(routes::telemetry::router())
        .route("/auth/rotate-token", axum::routing::post(routes::rotate_token))
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

    // Set socket permissions — owner-only access (backend runs as same user)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(SOCKET_PATH, std::fs::Permissions::from_mode(0o600)).ok();
    }

    // Load or generate the agent TLS cert now, so the fingerprint is available
    // for both the inbound TLS listener and the outbound phone-home payload.
    let (tls_config, cert_fingerprint) = match tls::load_or_generate().await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!("Failed to load/generate agent TLS cert: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!("Agent TLS cert fingerprint (SHA-256): {cert_fingerprint}");

    // Start phone-home if configured (remote agent mode)
    let remote_mode = if let Some(mut ph_config) = services::phone_home::PhoneHomeConfig::from_env() {
        ph_config.cert_fingerprint = Some(cert_fingerprint.clone());
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
            if let Err(e) = axum::serve(tcp_listener, tcp_app).await {
                tracing::error!("Remote-mode TCP server error: {e}");
            }
        });
    }

    // Multi-server: start TLS-wrapped TCP listener for remote panel connections.
    // Set AGENT_LISTEN_TCP=0.0.0.0:9443 to enable. The listener always terminates
    // TLS using the self-signed cert at /etc/dockpanel/ssl/agent.{crt,key}; the
    // central panel pins the cert's SHA-256 fingerprint on first checkin (TOFU).
    if let Ok(tcp_addr) = std::env::var("AGENT_LISTEN_TCP") {
        let parsed_addr: std::net::SocketAddr = match tcp_addr.parse() {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("AGENT_LISTEN_TCP {tcp_addr} is not a valid socket address: {e}");
                std::process::exit(1);
            }
        };
        let tcp_app = app.clone();
        let cfg = tls_config.clone();
        tokio::spawn(async move {
            tracing::info!("Agent TLS listener on {parsed_addr} (multi-server remote access)");
            if let Err(e) = axum_server::bind_rustls(parsed_addr, cfg)
                .serve(tcp_app.into_make_service())
                .await
            {
                tracing::error!("Multi-server TLS server error: {e}");
            }
        });
    }

    tracing::info!(
        "DockPanel Agent v{} listening on {SOCKET_PATH}",
        env!("CARGO_PKG_VERSION")
    );

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!("Agent server error: {e}");
    }

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
