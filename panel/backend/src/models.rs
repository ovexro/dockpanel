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

/// Phase 4 W3: rotation = ordered list of user IDs × cadence_days.
/// "Who's on-call at time T" is computed via cadence math against
/// `anchor_at` — no calendar, no per-day overrides.
///
/// Currently route handlers serialize ad-hoc DTOs that augment this with
/// resolved-member info, so this struct sits unused until the next caller
/// wants the raw row shape.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct OnCallSchedule {
    pub id: Uuid,
    pub name: String,
    pub members: Vec<Uuid>,
    pub cadence_days: i32,
    pub anchor_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Phase 4 W3: escalation policy = ordered JSONB array of `EscalationStep`s.
/// Referenced from `alert_rules.escalation_policy_id` (NULL = preserve
/// pre-W3 hardcoded 15/30-min behaviour).
///
/// Route handlers serialize ad-hoc DTOs that add `used_by_rule_count`, so
/// the raw row shape sits unused until another caller needs it.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EscalationPolicy {
    pub id: Uuid,
    pub name: String,
    pub steps: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One entry inside `escalation_policies.steps`. Stored as JSONB; decoded
/// when the alert engine evaluates the chain.
///
/// `route` is a discriminated string. Four valid shapes:
///   - `"on_call_schedule:<uuid>"` — resolve the schedule, page the current on-call user's channels.
///   - `"user:<uuid>"` — page a specific user's channels regardless of rotation.
///   - `"all_channels"` — fan out to every channel on the alert's owning `alert_rules` row.
///   - `"webhook:<url>"` — direct outbound webhook bypass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationStep {
    pub after_minutes: i32,
    pub route: String,
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
    /// Which ACME challenge issued the installed certificate: `http-01` or
    /// `dns-01`. NULL means "not recorded" — a row that predates the provenance
    /// migration and that the activity-log backfill could not prove. Renewal
    /// treats NULL as unknown and never as `http-01`; see
    /// [`crate::helpers::renewal_plan`].
    pub ssl_challenge: Option<String>,
    /// The name the certificate was actually ORDERED for. Equal to `domain`
    /// except for a wildcard, where it is the Cloudflare zone — the certificate
    /// covers `{subject}` and `*.{subject}`, never `*.{domain}`.
    pub ssl_cert_subject: Option<String>,
    /// Three-state on purpose: `Some(true)` wildcard, `Some(false)` not, `None`
    /// not recorded. A default `false` would assert "not a wildcard" over every
    /// legacy apex wildcard, which is exactly the row that cannot be identified.
    pub ssl_wildcard: Option<bool>,
    /// The `dns_zones` row whose Cloudflare credential issued this certificate.
    /// Renewal uses THIS row rather than re-deriving a zone from the subject, so
    /// the credential that renews is provably the credential that issued and no
    /// text match can select another tenant's token.
    pub ssl_dns_zone_id: Option<Uuid>,
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

/// Phase 4 W4: persistent panel snapshot (binaries + DB dump + /etc/dockpanel).
/// Survives past update.sh's `.bak` files (deleted on success at
/// `scripts/update.sh:499`) so an operator can roll back hours later.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PanelSnapshot {
    pub id: Uuid,
    pub file_path: String,
    pub from_version: String,
    pub to_version: Option<String>,
    pub trigger: String,
    pub operator: Option<String>,
    pub size_bytes: i64,
    pub sha256: String,
    pub rolled_back_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Phase 4 W4: fleet rolling-update run record. `plan` is the ordered
/// server list (JSONB array of `{server_id, name, agent_version}`);
/// `progress` is updated incrementally as the orchestrator walks the plan
/// (JSONB array of `{server_id, status, duration_ms, error?}`).
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FleetUpdateRun {
    pub id: Uuid,
    pub target_version: String,
    pub plan: serde_json::Value,
    pub progress: serde_json::Value,
    pub halt_on_failure: bool,
    pub include_panel: bool,
    pub started_by: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub outcome: Option<String>,
}

/// Phase 4 W4: update channel selector. Newtype around String so validation
/// is centralized — match arm rejects any value outside `stable | candidate
/// | hold`. Stored in `settings.update_channel` (single row).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateChannel(pub String);

impl UpdateChannel {
    pub fn validate(s: &str) -> Result<(), String> {
        match s {
            "stable" | "candidate" | "hold" => Ok(()),
            _ => Err(format!(
                "invalid channel '{s}' (must be one of: stable, candidate, hold)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_channel_validate_accepts_known_values() {
        assert!(UpdateChannel::validate("stable").is_ok());
        assert!(UpdateChannel::validate("candidate").is_ok());
        assert!(UpdateChannel::validate("hold").is_ok());
    }

    #[test]
    fn update_channel_validate_rejects_unknown_values() {
        assert!(UpdateChannel::validate("").is_err());
        assert!(UpdateChannel::validate("nightly").is_err());
        assert!(UpdateChannel::validate("STABLE").is_err());
        assert!(UpdateChannel::validate("stable ").is_err());
    }
}
