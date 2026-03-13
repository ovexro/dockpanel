use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::{is_valid_domain, AppState};
use crate::services;
use crate::services::ssl;

#[derive(Deserialize)]
pub struct SiteConfig {
    /// Site runtime: "static", "php", "proxy"
    pub runtime: String,
    /// Document root (for static/PHP) relative to site dir
    pub root: Option<String>,
    /// Upstream port (for proxy/Docker sites)
    pub proxy_port: Option<u16>,
    /// PHP-FPM socket path (for PHP sites)
    pub php_socket: Option<String>,
    /// Whether SSL is enabled
    pub ssl: Option<bool>,
    /// SSL certificate path
    pub ssl_cert: Option<String>,
    /// SSL key path
    pub ssl_key: Option<String>,
    /// Rate limit: requests per second per IP (None = no limit)
    pub rate_limit: Option<u32>,
    /// Max upload body size in MB
    pub max_upload_mb: Option<u32>,
    /// PHP memory_limit in MB (for PHP-FPM pool config)
    pub php_memory_mb: Option<u32>,
    /// PHP-FPM pm.max_children
    pub php_max_workers: Option<u32>,
}

#[derive(Serialize)]
struct NginxResponse {
    success: bool,
    message: String,
}

#[derive(Serialize)]
struct NginxTestResponse {
    success: bool,
    output: String,
}

#[derive(Serialize)]
struct SiteStatusResponse {
    domain: String,
    config_exists: bool,
    ssl_enabled: bool,
    ssl_cert_path: Option<String>,
    ssl_expiry: Option<String>,
}

/// PUT /nginx/sites/:domain — Create or update site nginx config.
async fn put_site(
    State(state): State<AppState>,
    Path(domain): Path<String>,
    Json(config): Json<SiteConfig>,
) -> Result<Json<NginxResponse>, (StatusCode, Json<NginxResponse>)> {
    // Validate domain format
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(NginxResponse {
                success: false,
                message: "Invalid domain format".into(),
            }),
        ));
    }

    // Write PHP-FPM pool config if PHP site with resource limits
    if config.runtime == "php" {
        if let Some(ref socket) = config.php_socket {
            // Extract PHP version from socket path (e.g., "unix:/run/php/php8.4-fpm.sock" → "8.4")
            if let Some(ver) = socket.strip_prefix("unix:/run/php/php").and_then(|s| s.strip_suffix("-fpm.sock")) {
                let memory = config.php_memory_mb.unwrap_or(256);
                let workers = config.php_max_workers.unwrap_or(5);
                if let Err(e) = services::nginx::write_php_pool_config(&domain, ver, memory, workers) {
                    tracing::warn!("Failed to write PHP pool config for {domain}: {e}");
                }
            }
        }
    }

    // Render nginx config from template
    let rendered = match services::nginx::render_site_config(&state.templates, &domain, &config) {
        Ok(c) => c,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(NginxResponse {
                    success: false,
                    message: format!("Template render error: {e}"),
                }),
            ));
        }
    };

    // Write config file
    let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
    if let Err(e) = std::fs::write(&config_path, &rendered) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(NginxResponse {
                success: false,
                message: format!("Failed to write config: {e}"),
            }),
        ));
    }

    // Test nginx config
    match services::nginx::test_config().await {
        Ok(output) if output.success => {
            // Reload nginx
            if let Err(e) = services::nginx::reload().await {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(NginxResponse {
                        success: false,
                        message: format!("Config valid but reload failed: {e}"),
                    }),
                ));
            }
            Ok(Json(NginxResponse {
                success: true,
                message: format!("Site {domain} configured and nginx reloaded"),
            }))
        }
        Ok(output) => {
            // Invalid config — remove it and restore
            std::fs::remove_file(&config_path).ok();
            Err((
                StatusCode::BAD_REQUEST,
                Json(NginxResponse {
                    success: false,
                    message: format!("Nginx config test failed: {}", output.stderr),
                }),
            ))
        }
        Err(e) => {
            std::fs::remove_file(&config_path).ok();
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(NginxResponse {
                    success: false,
                    message: format!("Failed to test config: {e}"),
                }),
            ))
        }
    }
}

/// DELETE /nginx/sites/:domain — Remove site nginx config.
async fn delete_site(
    Path(domain): Path<String>,
) -> Result<Json<NginxResponse>, (StatusCode, Json<NginxResponse>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(NginxResponse {
                success: false,
                message: "Invalid domain format".into(),
            }),
        ));
    }

    let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");

    if !std::path::Path::new(&config_path).exists() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(NginxResponse {
                success: false,
                message: format!("No config found for {domain}"),
            }),
        ));
    }

    if let Err(e) = std::fs::remove_file(&config_path) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(NginxResponse {
                success: false,
                message: format!("Failed to remove config: {e}"),
            }),
        ));
    }

    // Reload nginx
    if let Err(e) = services::nginx::reload().await {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(NginxResponse {
                success: false,
                message: format!("Config removed but reload failed: {e}"),
            }),
        ));
    }

    Ok(Json(NginxResponse {
        success: true,
        message: format!("Site {domain} removed and nginx reloaded"),
    }))
}

/// POST /nginx/test — Test nginx configuration.
async fn test_nginx() -> Json<NginxTestResponse> {
    match services::nginx::test_config().await {
        Ok(output) => Json(NginxTestResponse {
            success: output.success,
            output: if output.success {
                output.stdout
            } else {
                output.stderr
            },
        }),
        Err(e) => Json(NginxTestResponse {
            success: false,
            output: format!("Error: {e}"),
        }),
    }
}

/// POST /nginx/reload — Reload nginx.
async fn reload_nginx() -> Result<Json<NginxResponse>, (StatusCode, Json<NginxResponse>)> {
    match services::nginx::reload().await {
        Ok(_) => Ok(Json(NginxResponse {
            success: true,
            message: "Nginx reloaded".into(),
        })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(NginxResponse {
                success: false,
                message: format!("Reload failed: {e}"),
            }),
        )),
    }
}

/// GET /nginx/sites/:domain — Get site status.
async fn get_site(
    Path(domain): Path<String>,
) -> Result<Json<SiteStatusResponse>, (StatusCode, Json<NginxResponse>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(NginxResponse {
                success: false,
                message: "Invalid domain format".into(),
            }),
        ));
    }

    let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
    let ssl_cert_path = format!("/etc/dockpanel/ssl/{domain}/fullchain.pem");
    let config_exists = std::path::Path::new(&config_path).exists();
    let ssl_enabled = std::path::Path::new(&ssl_cert_path).exists();

    let ssl_expiry = if ssl_enabled {
        let status = ssl::get_cert_status(&domain).await;
        status.not_after
    } else {
        None
    };

    Ok(Json(SiteStatusResponse {
        domain,
        config_exists,
        ssl_enabled,
        ssl_cert_path: if ssl_enabled {
            Some(ssl_cert_path)
        } else {
            None
        },
        ssl_expiry,
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/nginx/sites/{domain}", put(put_site))
        .route("/nginx/sites/{domain}", delete(delete_site))
        .route("/nginx/sites/{domain}", get(get_site))
        .route("/nginx/test", post(test_nginx))
        .route("/nginx/reload", post(reload_nginx))
}
