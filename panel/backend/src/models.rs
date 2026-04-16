use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[allow(dead_code)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: String,
    pub email_verified: bool,
    #[serde(skip_serializing)]
    pub email_token: Option<String>,
    #[serde(skip_serializing)]
    pub reset_token: Option<String>,
    #[serde(skip_serializing)]
    pub reset_expires: Option<DateTime<Utc>>,
    #[serde(skip_serializing)]
    pub stripe_customer_id: Option<String>,
    #[serde(skip_serializing)]
    pub stripe_subscription_id: Option<String>,
    pub plan: String,
    pub plan_status: String,
    pub plan_server_limit: i32,
    #[serde(skip_serializing)]
    pub totp_secret: Option<String>,
    pub totp_enabled: bool,
    #[serde(skip_serializing)]
    pub recovery_codes: Option<String>,
    pub oauth_provider: Option<String>,
    #[serde(skip_serializing)]
    pub oauth_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Site {
    pub id: Uuid,
    pub user_id: Uuid,
    pub server_id: Option<Uuid>,
    pub domain: String,
    pub runtime: String,
    pub status: String,
    pub proxy_port: Option<i32>,
    pub php_version: Option<String>,
    pub root_path: Option<String>,
    pub ssl_enabled: bool,
    pub ssl_cert_path: Option<String>,
    pub ssl_key_path: Option<String>,
    pub ssl_expiry: Option<DateTime<Utc>>,
    pub ssl_profile: Option<String>,
    pub ssl_renewal_at: Option<DateTime<Utc>>,
    pub ssl_renewal_checked_at: Option<DateTime<Utc>>,
    pub rate_limit: Option<i32>,
    pub max_upload_mb: i32,
    pub php_memory_mb: i32,
    pub php_max_workers: i32,
    pub custom_nginx: Option<String>,
    pub php_preset: Option<String>,
    pub app_command: Option<String>,
    pub parent_site_id: Option<Uuid>,
    pub synced_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub fastcgi_cache: bool,
    pub redis_cache: bool,
    pub redis_db: i32,
    pub waf_enabled: bool,
    pub waf_mode: String,
    pub csp_policy: Option<String>,
    pub permissions_policy: Option<String>,
    pub bot_protection: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
