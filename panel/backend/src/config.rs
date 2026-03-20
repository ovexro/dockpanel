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
