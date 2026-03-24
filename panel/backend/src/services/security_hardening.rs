//! Security Hardening Service — post-incident enhancements.
//!
//! Provides:
//! - Geo-IP lookup for login/register alerting (Feature 1)
//! - Immutable security audit log writes (Feature 7)
//! - Suspicious event tracking + auto-lockdown (Feature 4/9)
//! - Lockdown state management (Feature 9/11)
//! - Tamper-resistant file logging (Feature 6)

use sqlx::PgPool;
use std::sync::OnceLock;
use std::time::Duration;

/// Shared HTTP client for geo-IP lookups (reuses connections).
fn geo_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default()
    })
}

// ── Geo-IP Lookup (Feature 1) ───────────────────────────────────────────

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct GeoInfo {
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub isp: String,
    #[serde(default)]
    pub proxy: bool,
    #[serde(default)]
    pub hosting: bool,
    #[serde(default)]
    pub query: String,
}

/// Look up geo-IP information for an IP address.
/// Uses ip-api.com free tier (45 req/min, no key needed).
/// Returns None on failure (non-blocking, best-effort).
pub async fn lookup_geo_ip(ip: &str) -> Option<GeoInfo> {
    // Skip private/local IPs
    if ip == "unknown" || ip.starts_with("192.168.") || ip.starts_with("10.")
        || ip.starts_with("172.") || ip == "127.0.0.1" || ip == "::1"
    {
        return Some(GeoInfo {
            country: "Local".into(),
            city: "LAN".into(),
            isp: "Private Network".into(),
            query: ip.into(),
            ..Default::default()
        });
    }

    let url = format!(
        "http://ip-api.com/json/{ip}?fields=status,country,city,isp,proxy,hosting,query"
    );

    let resp = geo_client().get(&url).send().await.ok()?;
    let data: serde_json::Value = resp.json().await.ok()?;

    if data.get("status").and_then(|s| s.as_str()) != Some("success") {
        return None;
    }

    Some(GeoInfo {
        country: data["country"].as_str().unwrap_or("").into(),
        city: data["city"].as_str().unwrap_or("").into(),
        isp: data["isp"].as_str().unwrap_or("").into(),
        proxy: data["proxy"].as_bool().unwrap_or(false),
        hosting: data["hosting"].as_bool().unwrap_or(false),
        query: data["query"].as_str().unwrap_or(ip).into(),
    })
}

// ── Immutable Audit Log (Feature 7) ─────────────────────────────────────

/// Write to the immutable security audit log.
/// This table has a PostgreSQL trigger preventing UPDATE/DELETE.
pub async fn audit_log(
    pool: &PgPool,
    event_type: &str,
    actor_email: Option<&str>,
    actor_ip: Option<&str>,
    target_type: Option<&str>,
    target_name: Option<&str>,
    details: Option<&str>,
    geo: Option<&GeoInfo>,
    severity: &str,
) {
    let (country, city, isp) = geo.map(|g| {
        (Some(g.country.as_str()), Some(g.city.as_str()), Some(g.isp.as_str()))
    }).unwrap_or((None, None, None));

    if let Err(e) = sqlx::query(
        "INSERT INTO security_audit_log \
         (event_type, actor_email, actor_ip, target_type, target_name, details, \
          geo_country, geo_city, geo_isp, severity) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"
    )
    .bind(event_type)
    .bind(actor_email)
    .bind(actor_ip)
    .bind(target_type)
    .bind(target_name)
    .bind(details)
    .bind(country)
    .bind(city)
    .bind(isp)
    .bind(severity)
    .execute(pool)
    .await {
        tracing::warn!("Failed to write security audit log: {e}");
    }

    // Feature 6: Also write to tamper-resistant file log
    write_to_audit_file(event_type, actor_email, actor_ip, details, severity);
}

// ── Tamper-Resistant File Logging (Feature 6) ───────────────────────────

/// Write audit entry to append-only file on disk.
/// The directory /var/lib/dockpanel/audit/ should have chattr +a set.
fn write_to_audit_file(
    event_type: &str,
    actor_email: Option<&str>,
    actor_ip: Option<&str>,
    details: Option<&str>,
    severity: &str,
) {
    use std::io::Write;

    let dir = "/var/lib/dockpanel/audit";
    let _ = std::fs::create_dir_all(dir);

    let date = chrono::Utc::now().format("%Y-%m-%d");
    let path = format!("{dir}/audit-{date}.log");

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let ts = chrono::Utc::now().to_rfc3339();
        let email = actor_email.unwrap_or("-");
        let ip = actor_ip.unwrap_or("-");
        let det = details.unwrap_or("-");
        let _ = writeln!(file, "{ts}\t{severity}\t{event_type}\t{email}\t{ip}\t{det}");
    }
}

// ── Suspicious Event Tracking (Feature 4/9) ─────────────────────────────

/// Record a suspicious event and check if auto-lockdown threshold is reached.
/// Returns true if lockdown was triggered.
pub async fn record_suspicious_event(
    pool: &PgPool,
    event_type: &str,
    actor_email: Option<&str>,
    actor_ip: Option<&str>,
    details: Option<&str>,
) -> bool {
    // Record the event
    let _ = sqlx::query(
        "INSERT INTO suspicious_events (event_type, actor_email, actor_ip, details) \
         VALUES ($1, $2, $3, $4)"
    )
    .bind(event_type)
    .bind(actor_email)
    .bind(actor_ip)
    .bind(details)
    .execute(pool)
    .await;

    // Check threshold
    let threshold: i64 = get_setting_i64(pool, "security_lockdown_threshold", 5).await;
    let window_mins: i64 = get_setting_i64(pool, "security_lockdown_window_minutes", 10).await;

    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM suspicious_events \
         WHERE created_at > NOW() - make_interval(mins => $1)"
    )
    .bind(window_mins as i32)
    .fetch_one(pool)
    .await
    .unwrap_or((0,));

    if count.0 >= threshold {
        // Check if already locked down
        let locked: Option<(bool,)> = sqlx::query_as(
            "SELECT active FROM lockdown_state WHERE id = 1"
        ).fetch_optional(pool).await.ok().flatten();

        if locked.map(|(a,)| !a).unwrap_or(true) {
            // Trigger auto-lockdown
            let reason = format!(
                "Auto-lockdown: {} suspicious events in {} minutes (threshold: {})",
                count.0, window_mins, threshold
            );
            activate_lockdown(pool, "auto", &reason).await;

            audit_log(
                pool, "lockdown.auto", None, None,
                Some("system"), None,
                Some(&reason), None, "critical",
            ).await;

            return true;
        }
    }

    false
}

// ── Lockdown Management (Feature 9/11) ──────────────────────────────────

/// Activate lockdown mode.
pub async fn activate_lockdown(pool: &PgPool, triggered_by: &str, reason: &str) {
    let _ = sqlx::query(
        "UPDATE lockdown_state SET \
         active = TRUE, triggered_by = $1, triggered_at = NOW(), reason = $2, \
         terminals_disabled = TRUE, registration_disabled = TRUE, non_admin_blocked = TRUE, \
         unlocked_at = NULL, unlocked_by = NULL \
         WHERE id = 1"
    )
    .bind(triggered_by)
    .bind(reason)
    .execute(pool)
    .await;

    tracing::error!("🔒 LOCKDOWN ACTIVATED: {triggered_by} — {reason}");
}

/// Deactivate lockdown mode (admin only).
pub async fn deactivate_lockdown(pool: &PgPool, admin_email: &str) {
    let _ = sqlx::query(
        "UPDATE lockdown_state SET \
         active = FALSE, unlocked_at = NOW(), unlocked_by = $1 \
         WHERE id = 1"
    )
    .bind(admin_email)
    .execute(pool)
    .await;

    tracing::info!("🔓 LOCKDOWN DEACTIVATED by {admin_email}");
}

/// Check if system is currently in lockdown.
pub async fn is_locked_down(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT active FROM lockdown_state WHERE id = 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// Get lockdown state details.
pub async fn get_lockdown_state(pool: &PgPool) -> serde_json::Value {
    let row: Option<(bool, Option<String>, Option<chrono::DateTime<chrono::Utc>>, Option<String>)> =
        sqlx::query_as(
            "SELECT active, triggered_by, triggered_at, reason FROM lockdown_state WHERE id = 1"
        )
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

    match row {
        Some((active, triggered_by, triggered_at, reason)) => serde_json::json!({
            "active": active,
            "triggered_by": triggered_by,
            "triggered_at": triggered_at,
            "reason": reason,
        }),
        None => serde_json::json!({ "active": false }),
    }
}

// ── Geo-IP Alert Notification (Feature 1) ───────────────────────────────

/// Send an alert to all admin notification channels about a suspicious login/register.
pub async fn alert_suspicious_ip(
    pool: &PgPool,
    event: &str,      // "login" or "register"
    email: &str,
    ip: &str,
    geo: &GeoInfo,
) {
    let proxy_tag = if geo.proxy || geo.hosting { " ⚠️ PROXY/VPN/DATACENTER" } else { "" };
    let subject = format!(
        "🚨 Security Alert: {} from new IP{}",
        event, proxy_tag
    );
    let message = format!(
        "User: {email}\nIP: {ip}\nCountry: {}\nCity: {}\nISP: {}{}\nTime: {}",
        geo.country, geo.city, geo.isp, proxy_tag,
        chrono::Utc::now().to_rfc3339()
    );
    let html = format!(
        "<h2>Security Alert: {event}</h2>\
         <p><strong>User:</strong> {email}</p>\
         <p><strong>IP:</strong> {ip}</p>\
         <p><strong>Location:</strong> {}, {}</p>\
         <p><strong>ISP:</strong> {}{}</p>\
         <p><strong>Time:</strong> {}</p>",
        geo.country, geo.city, geo.isp, proxy_tag,
        chrono::Utc::now().to_rfc3339()
    );

    // Send to all admin users' notification channels
    let admins: Vec<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM users WHERE role = 'admin'"
    ).fetch_all(pool).await.unwrap_or_default();

    for (admin_id,) in admins {
        if let Some(channels) = super::notifications::get_user_channels(pool, admin_id, None).await {
            super::notifications::send_notification(pool, &channels, &subject, &message, &html).await;
        }
    }

    // Also create a panel notification for all admins
    for (admin_id,) in sqlx::query_as::<_, (uuid::Uuid,)>(
        "SELECT id FROM users WHERE role = 'admin'"
    ).fetch_all(pool).await.unwrap_or_default() {
        let _ = sqlx::query(
            "INSERT INTO panel_notifications (user_id, title, message, severity, category) \
             VALUES ($1, $2, $3, $4, 'security')"
        )
        .bind(admin_id)
        .bind(&subject)
        .bind(&message)
        .bind(if geo.proxy || geo.hosting { "critical" } else { "warning" })
        .execute(pool)
        .await;
    }
}

/// Send an emergency lockdown notification to all admin channels.
pub async fn alert_lockdown(pool: &PgPool, reason: &str, triggered_by: &str) {
    let subject = "🔒 EMERGENCY: System Lockdown Activated";
    let message = format!(
        "DockPanel has entered lockdown mode.\n\
         Triggered by: {triggered_by}\n\
         Reason: {reason}\n\
         All terminals disabled. Registration blocked.\n\
         Admin action required to unlock."
    );
    let html = format!(
        "<h2 style='color:red'>🔒 System Lockdown</h2>\
         <p><strong>Triggered by:</strong> {triggered_by}</p>\
         <p><strong>Reason:</strong> {reason}</p>\
         <p>All terminals disabled. Registration blocked. Admin action required.</p>"
    );

    let admins: Vec<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM users WHERE role = 'admin'"
    ).fetch_all(pool).await.unwrap_or_default();

    for (admin_id,) in &admins {
        if let Some(channels) = super::notifications::get_user_channels(pool, *admin_id, None).await {
            super::notifications::send_notification(pool, &channels, subject, &message, &html).await;
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

async fn get_setting_i64(pool: &PgPool, key: &str, default: i64) -> i64 {
    sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(default)
}

pub async fn get_setting_bool(pool: &PgPool, key: &str, default: bool) -> bool {
    sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(default)
}
