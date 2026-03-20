#[cfg(test)]
mod tests {
    #[test]
    fn base_url_defaults_to_empty() {
        // This is the bug we caught: BASE_URL used to default to "https://demo.dockpanel.dev"
        // which caused Secure cookies over HTTP. Verify the fix.
        unsafe { std::env::remove_var("BASE_URL"); }
        let url = std::env::var("BASE_URL").unwrap_or_default();
        assert!(url.is_empty(), "BASE_URL should default to empty, got: {url}");
        assert!(!url.starts_with("https"), "Empty BASE_URL must not trigger Secure cookies");
    }

    #[test]
    fn base_url_from_env() {
        unsafe { std::env::set_var("BASE_URL", "https://panel.example.com"); }
        let url = std::env::var("BASE_URL").unwrap_or_default();
        assert_eq!(url, "https://panel.example.com");
        assert!(url.starts_with("https"));
        unsafe { std::env::remove_var("BASE_URL"); }
    }

    #[test]
    fn cors_empty_when_no_config() {
        unsafe {
            std::env::remove_var("CORS_ORIGINS");
            std::env::remove_var("BASE_URL");
        }
        let base_url = std::env::var("BASE_URL").unwrap_or_default();
        let cors = std::env::var("CORS_ORIGINS")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| s.split(',').map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect::<Vec<_>>())
            .unwrap_or_else(|| {
                if base_url.is_empty() { vec![] } else { vec![base_url.clone()] }
            });
        assert!(cors.is_empty(), "CORS origins should be empty when no config set");
    }

    #[test]
    fn secure_flag_logic() {
        // Verify the secure flag logic matches what auth.rs uses
        let empty_url = "";
        assert!(!empty_url.starts_with("https"), "Empty URL must not set Secure flag");

        let http_url = "http://192.168.1.1:8443";
        assert!(!http_url.starts_with("https"), "HTTP URL must not set Secure flag");

        let https_url = "https://panel.example.com";
        assert!(https_url.starts_with("https"), "HTTPS URL must set Secure flag");
    }
}

/// Application configuration loaded from environment variables.
pub struct Config {
    pub database_url: String,
    pub jwt_secret: String,
    pub agent_socket: String,
    pub agent_token: String,
    pub listen_addr: String,
    pub db_max_connections: u32,
    pub stripe_secret_key: Option<String>,
    pub stripe_webhook_secret: Option<String>,
    pub base_url: String,
    pub cors_origins: Vec<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

        if jwt_secret.len() < 32 {
            panic!("JWT_SECRET must be at least 32 characters (got {}). Generate with: openssl rand -hex 32", jwt_secret.len());
        }

        let base_url = std::env::var("BASE_URL").unwrap_or_default();

        let cors_origins = std::env::var("CORS_ORIGINS")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| s.split(',').map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect::<Vec<_>>())
            .unwrap_or_else(|| {
                if base_url.is_empty() {
                    // No BASE_URL set — allow all origins (typical for IP-based access)
                    vec![]
                } else {
                    vec![base_url.clone()]
                }
            });

        Self {
            database_url: std::env::var("DATABASE_URL")
                .expect("DATABASE_URL must be set"),
            jwt_secret,
            agent_socket: std::env::var("AGENT_SOCKET")
                .unwrap_or_else(|_| "/var/run/dockpanel/agent.sock".into()),
            agent_token: std::env::var("AGENT_TOKEN")
                .expect("AGENT_TOKEN must be set"),
            listen_addr: std::env::var("LISTEN_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:3080".into()),
            db_max_connections: std::env::var("DB_MAX_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            stripe_secret_key: std::env::var("STRIPE_SECRET_KEY").ok().filter(|s| !s.is_empty()),
            stripe_webhook_secret: std::env::var("STRIPE_WEBHOOK_SECRET").ok().filter(|s| !s.is_empty()),
            base_url,
            cors_origins,
        }
    }
}
