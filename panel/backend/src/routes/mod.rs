pub mod activity;
pub mod agent_checkin;
pub mod agent_commands;
pub mod alerts;
pub mod agent_updates;
pub mod api_keys;
pub mod auth;
pub mod backup_destinations;
pub mod dashboard;
pub mod backup_schedules;
pub mod backups;
pub mod billing;
pub mod crons;
pub mod databases;
pub mod deploy;
pub mod dns;
pub mod docker_apps;
pub mod files;
pub mod logs;
pub mod mail;
pub mod metrics;
pub mod monitors;
pub mod security;
pub mod security_scans;
pub mod server_actions;
pub mod servers;
pub mod settings;
pub mod staging;
pub mod sites;
pub mod teams;
pub mod ssl;
pub mod system;
pub mod terminal;
pub mod users;
pub mod wordpress;

use axum::{
    routing::{delete, get, post, put},
    Router,
};

use crate::AppState;

/// Validate a domain name format.
pub fn is_valid_domain(domain: &str) -> bool {
    if domain.is_empty() || domain.len() > 253 {
        return false;
    }
    domain.split('.').all(|part| {
        !part.is_empty()
            && part.len() <= 63
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !part.starts_with('-')
            && !part.ends_with('-')
    }) && domain.contains('.')
}

/// Validate a resource name (database, app, etc.).
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validate a Docker container ID (hex string, 1–64 chars).
pub fn is_valid_container_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// Validate a user-provided file path (reject traversal and injection).
pub fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        && !path.contains("..")
        && !path.starts_with('/')
}

pub fn router() -> Router<AppState> {
    Router::new()
        // Auth
        .route("/api/auth/setup", post(auth::setup))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/register", post(auth::register))
        .route("/api/auth/verify-email", post(auth::verify_email))
        .route("/api/auth/forgot-password", post(auth::forgot_password))
        .route("/api/auth/reset-password", post(auth::reset_password))
        // Two-Factor Authentication
        .route("/api/auth/2fa/setup", post(auth::twofa_setup))
        .route("/api/auth/2fa/enable", post(auth::twofa_enable))
        .route("/api/auth/2fa/verify", post(auth::twofa_verify))
        .route("/api/auth/2fa/disable", post(auth::twofa_disable))
        .route("/api/auth/2fa/status", get(auth::twofa_status))
        // Users (admin)
        .route("/api/users", get(users::list).post(users::create))
        .route("/api/users/{id}", put(users::update).delete(users::remove))
        // Sites
        .route("/api/sites", get(sites::list).post(sites::create))
        .route("/api/sites/{id}", get(sites::get_one).delete(sites::remove))
        .route("/api/sites/{id}/php", put(sites::switch_php))
        .route("/api/sites/{id}/limits", put(sites::update_limits))
        // PHP versions
        .route("/api/php/versions", get(sites::php_versions))
        .route("/api/php/install", post(sites::php_install))
        // SSL
        .route("/api/sites/{id}/ssl", post(ssl::provision).get(ssl::status))
        // File Manager
        .route("/api/sites/{id}/files", get(files::list_dir).delete(files::delete_entry))
        .route("/api/sites/{id}/files/read", get(files::read_file))
        .route("/api/sites/{id}/files/write", put(files::write_file))
        .route("/api/sites/{id}/files/create", post(files::create_entry))
        .route("/api/sites/{id}/files/rename", post(files::rename_entry))
        // Backups
        .route("/api/sites/{id}/backups", get(backups::list).post(backups::create))
        .route("/api/sites/{id}/backups/{backup_id}/restore", post(backups::restore))
        .route("/api/sites/{id}/backups/{backup_id}", delete(backups::remove))
        // Backup Destinations (admin)
        .route("/api/backup-destinations", get(backup_destinations::list).post(backup_destinations::create))
        .route("/api/backup-destinations/{id}", put(backup_destinations::update).delete(backup_destinations::remove))
        .route("/api/backup-destinations/{id}/test", post(backup_destinations::test_connection))
        // Backup Schedules
        .route("/api/sites/{id}/backup-schedule", get(backup_schedules::get_schedule).put(backup_schedules::set_schedule).delete(backup_schedules::remove_schedule))
        // Crons
        .route("/api/sites/{id}/crons", get(crons::list).post(crons::create))
        .route("/api/sites/{id}/crons/{cron_id}", put(crons::update).delete(crons::remove))
        .route("/api/sites/{id}/crons/{cron_id}/run", post(crons::run_now))
        // Site Logs
        .route("/api/sites/{id}/logs", get(logs::site_logs))
        .route("/api/sites/{id}/logs/search", get(logs::search_site_logs))
        // Terminal
        .route("/api/terminal/token", get(terminal::ws_token))
        // Databases
        .route("/api/databases", get(databases::list).post(databases::create))
        .route("/api/databases/{id}", delete(databases::remove))
        .route("/api/databases/{id}/credentials", get(databases::credentials))
        // Docker Apps (admin)
        .route("/api/apps/templates", get(docker_apps::list_templates))
        .route("/api/apps/deploy", post(docker_apps::deploy))
        .route("/api/apps/compose/parse", post(docker_apps::compose_parse))
        .route("/api/apps/compose/deploy", post(docker_apps::compose_deploy))
        .route("/api/apps", get(docker_apps::list_apps))
        .route("/api/apps/{container_id}", delete(docker_apps::remove_app))
        .route("/api/apps/{container_id}/stop", post(docker_apps::stop_app))
        .route("/api/apps/{container_id}/start", post(docker_apps::start_app))
        .route("/api/apps/{container_id}/restart", post(docker_apps::restart_app))
        .route("/api/apps/{container_id}/logs", get(docker_apps::app_logs))
        .route("/api/apps/{container_id}/env", get(docker_apps::app_env))
        .route("/api/apps/{container_id}/update", post(docker_apps::update_app))
        // Security (admin)
        .route("/api/security/overview", get(security::overview))
        .route("/api/security/firewall", get(security::firewall_status))
        .route("/api/security/firewall/rules", post(security::add_firewall_rule))
        .route("/api/security/firewall/rules/{number}", delete(security::delete_firewall_rule))
        .route("/api/security/fail2ban", get(security::fail2ban_status))
        // Security Scanning
        .route("/api/security/scan", post(security_scans::trigger_scan))
        .route("/api/security/scans", get(security_scans::list_scans))
        .route("/api/security/scans/{id}", get(security_scans::get_scan))
        .route("/api/security/posture", get(security_scans::posture))
        // System
        .route("/api/health", get(system::health))
        .route("/api/system/info", get(system::info))
        .route("/api/system/processes", get(logs::processes))
        .route("/api/system/network", get(logs::network))
        // System Updates
        .route("/api/system/updates", get(system::updates_list))
        .route("/api/system/updates/apply", post(system::updates_apply))
        .route("/api/system/updates/count", get(system::updates_count))
        .route("/api/system/reboot", post(system::system_reboot))
        // Logs (admin)
        .route("/api/logs", get(logs::system_logs))
        .route("/api/logs/search", get(logs::search_system_logs))
        .route("/api/logs/stream/token", get(logs::stream_token))
        // Settings (admin)
        .route("/api/settings", get(settings::list).put(settings::update))
        .route("/api/settings/smtp/test", post(settings::test_email))
        .route("/api/settings/test-webhook", post(settings::test_webhook))
        .route("/api/settings/health", get(settings::health))
        // DNS Management
        .route("/api/dns/zones", get(dns::list_zones).post(dns::create_zone))
        .route("/api/dns/zones/{id}", delete(dns::delete_zone))
        .route("/api/dns/zones/{id}/records", get(dns::list_records).post(dns::create_record))
        .route("/api/dns/zones/{id}/records/{record_id}", put(dns::update_record).delete(dns::delete_record))
        // WordPress Management
        .route("/api/sites/{id}/wordpress", get(wordpress::info))
        .route("/api/sites/{id}/wordpress/install", post(wordpress::install))
        .route("/api/sites/{id}/wordpress/plugins", get(wordpress::plugins))
        .route("/api/sites/{id}/wordpress/themes", get(wordpress::themes))
        .route("/api/sites/{id}/wordpress/update/{target}", post(wordpress::update))
        .route("/api/sites/{id}/wordpress/plugin/{action}", post(wordpress::plugin_action))
        .route("/api/sites/{id}/wordpress/theme/{action}", post(wordpress::theme_action))
        .route("/api/sites/{id}/wordpress/auto-update", post(wordpress::set_auto_update))
        // Git Deploy
        .route("/api/sites/{id}/deploy", get(deploy::get_config).put(deploy::set_config).delete(deploy::remove_config))
        .route("/api/sites/{id}/deploy/trigger", post(deploy::trigger))
        .route("/api/sites/{id}/deploy/keygen", post(deploy::keygen))
        .route("/api/sites/{id}/deploy/logs", get(deploy::logs))
        // Uptime Monitors
        .route("/api/monitors", get(monitors::list).post(monitors::create))
        .route("/api/monitors/{id}", put(monitors::update).delete(monitors::remove))
        .route("/api/monitors/{id}/checks", get(monitors::checks))
        .route("/api/monitors/{id}/incidents", get(monitors::incidents))
        // Billing
        .route("/api/billing/plan", get(billing::current_plan))
        .route("/api/billing/checkout", post(billing::create_checkout))
        .route("/api/billing/portal", post(billing::customer_portal))
        // Webhooks (no auth — validated by secret/signature)
        .route("/api/webhooks/stripe", post(billing::webhook))
        .route("/api/webhooks/deploy/{site_id}/{secret}", post(deploy::webhook))
        // Staging Environments
        .route("/api/sites/{id}/staging", get(staging::get_staging).post(staging::create).delete(staging::destroy))
        .route("/api/sites/{id}/staging/sync", post(staging::sync_to_staging))
        .route("/api/sites/{id}/staging/push", post(staging::push_to_prod))
        // Agent endpoints (no cookie auth — uses Bearer token from servers table)
        .route("/api/agent/version", get(agent_updates::latest_version))
        .route("/api/agent/checkin", post(agent_checkin::checkin))
        .route("/api/agent/commands", get(agent_commands::poll))
        .route("/api/agent/commands/result", post(agent_commands::report_result))
        // API Keys
        .route("/api/api-keys", get(api_keys::list).post(api_keys::create))
        .route("/api/api-keys/{id}", delete(api_keys::revoke))
        // Servers
        .route("/api/servers", get(servers::list).post(servers::create))
        .route("/api/servers/{id}", get(servers::get_one).delete(servers::remove))
        .route("/api/servers/{id}/metrics", get(metrics::server_metrics))
        .route("/api/servers/{id}/commands", post(server_actions::dispatch).get(server_actions::list_commands))
        .route("/api/servers/{id}/commands/{cmd_id}", get(server_actions::command_status))
        // Teams
        .route("/api/teams", get(teams::list).post(teams::create))
        .route("/api/teams/{id}", delete(teams::remove))
        .route("/api/teams/{id}/invite", post(teams::invite))
        .route("/api/teams/{id}/members/{member_id}", put(teams::update_member).delete(teams::remove_member))
        .route("/api/teams/accept", post(teams::accept_invite))
        // Alerts
        .route("/api/alerts", get(alerts::list))
        .route("/api/alerts/summary", get(alerts::summary))
        .route("/api/alerts/{id}/acknowledge", put(alerts::acknowledge))
        .route("/api/alerts/{id}/resolve", put(alerts::resolve))
        .route("/api/alert-rules", get(alerts::get_rules).put(alerts::update_rules))
        .route("/api/alert-rules/{server_id}", put(alerts::update_server_rules).delete(alerts::delete_server_rules))
        // Dashboard Intelligence
        .route("/api/dashboard/intelligence", get(dashboard::intelligence))
        .route("/api/dashboard/metrics-history", get(dashboard::metrics_history))
        // SSH Keys
        .route("/api/ssh-keys", get(system::list_ssh_keys).post(system::add_ssh_key))
        .route("/api/ssh-keys/{fingerprint}", delete(system::remove_ssh_key))
        // Auto-Updates
        .route("/api/auto-updates/status", get(system::auto_updates_status))
        .route("/api/auto-updates/enable", post(system::enable_auto_updates))
        .route("/api/auto-updates/disable", post(system::disable_auto_updates))
        // Panel IP Whitelist
        .route("/api/panel-whitelist", get(system::get_panel_whitelist).post(system::set_panel_whitelist))
        // Service installers
        .route("/api/services/install-status", get(system::install_status))
        .route("/api/services/install/php", post(system::install_php))
        .route("/api/services/install/certbot", post(system::install_certbot))
        .route("/api/services/install/ufw", post(system::install_ufw))
        .route("/api/services/install/fail2ban", post(system::install_fail2ban))
        // Mail
        .route("/api/mail/status", get(mail::mail_status))
        .route("/api/mail/install", post(mail::mail_install))
        .route("/api/mail/domains", get(mail::list_domains).post(mail::create_domain))
        .route("/api/mail/domains/{id}", put(mail::update_domain).delete(mail::delete_domain))
        .route("/api/mail/domains/{id}/dns", get(mail::domain_dns))
        .route("/api/mail/domains/{id}/accounts", get(mail::list_accounts).post(mail::create_account))
        .route("/api/mail/domains/{id}/accounts/{account_id}", put(mail::update_account).delete(mail::delete_account))
        .route("/api/mail/domains/{id}/aliases", get(mail::list_aliases).post(mail::create_alias))
        .route("/api/mail/domains/{id}/aliases/{alias_id}", delete(mail::delete_alias))
        .route("/api/mail/queue", get(mail::get_queue))
        .route("/api/mail/queue/flush", post(mail::flush_queue))
        .route("/api/mail/queue/{queue_id}", delete(mail::delete_queued))
        // Agent Diagnostics proxy
        .route("/api/agent/diagnostics", get(system::diagnostics))
        .route("/api/agent/diagnostics/fix", post(system::diagnostics_fix))
        // Activity (admin)
        .route("/api/activity", get(activity::list))
}
