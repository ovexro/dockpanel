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
pub mod git_deploys;
pub mod logs;
pub mod mail;
pub mod metrics;
pub mod monitors;
pub mod security;
pub mod security_scans;
pub mod server_actions;
pub mod servers;
pub mod settings;
pub mod stacks;
pub mod staging;
pub mod sites;
pub mod system_logs;
pub mod teams;
pub mod ssl;
pub mod system;
pub mod terminal;
pub mod users;
pub mod wordpress;
pub mod ws_metrics;

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
        .route("/api/auth/setup-status", get(auth::setup_status))
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
        .route("/api/sites/{id}/provision-log", get(sites::provision_log))
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
        .route("/api/sites/{id}/files/download", get(files::download_file))
        .route("/api/sites/{id}/files/write", put(files::write_file))
        .route("/api/sites/{id}/files/upload", post(files::upload_file))
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
        .route("/api/databases/{id}/tables", get(databases::tables))
        .route("/api/databases/{id}/tables/{table}", get(databases::table_schema))
        .route("/api/databases/{id}/query", post(databases::query))
        // Compose Stacks
        .route("/api/stacks", get(stacks::list).post(stacks::create))
        .route("/api/stacks/{id}", get(stacks::get_one).put(stacks::update).delete(stacks::remove))
        .route("/api/stacks/{id}/start", post(stacks::start))
        .route("/api/stacks/{id}/stop", post(stacks::stop))
        .route("/api/stacks/{id}/restart", post(stacks::restart))
        // Docker Apps (admin)
        .route("/api/apps/templates", get(docker_apps::list_templates))
        .route("/api/apps/deploy", post(docker_apps::deploy))
        .route("/api/apps/deploy/{deploy_id}/log", get(docker_apps::deploy_log))
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
        // Git Deploy
        .route("/api/git-deploys", get(git_deploys::list).post(git_deploys::create))
        .route("/api/git-deploys/{id}", get(git_deploys::get_one).put(git_deploys::update).delete(git_deploys::remove))
        .route("/api/git-deploys/{id}/deploy", post(git_deploys::deploy))
        .route("/api/git-deploys/{id}/rollback/{history_id}", post(git_deploys::rollback))
        .route("/api/git-deploys/{id}/history", get(git_deploys::history))
        .route("/api/git-deploys/{id}/keygen", post(git_deploys::keygen))
        .route("/api/git-deploys/{id}/stop", post(git_deploys::stop))
        .route("/api/git-deploys/{id}/start", post(git_deploys::start))
        .route("/api/git-deploys/{id}/restart", post(git_deploys::restart))
        .route("/api/git-deploys/{id}/logs", get(git_deploys::container_logs))
        .route("/api/git-deploys/{id}/previews", get(git_deploys::list_previews))
        .route("/api/git-deploys/{id}/previews/{preview_id}", delete(git_deploys::delete_preview))
        .route("/api/git-deploys/deploy/{deploy_id}/log", get(git_deploys::deploy_log))
        .route("/api/webhooks/git/{id}/{secret}", post(git_deploys::webhook))
        // Security (admin)
        .route("/api/security/overview", get(security::overview))
        .route("/api/security/firewall", get(security::firewall_status))
        .route("/api/security/firewall/rules", post(security::add_firewall_rule))
        .route("/api/security/firewall/rules/{number}", delete(security::delete_firewall_rule))
        .route("/api/security/fail2ban", get(security::fail2ban_status))
        // SSH Hardening
        .route("/api/security/ssh/disable-password", post(security::ssh_disable_password))
        .route("/api/security/ssh/enable-password", post(security::ssh_enable_password))
        .route("/api/security/ssh/disable-root", post(security::ssh_disable_root))
        .route("/api/security/ssh/change-port", post(security::ssh_change_port))
        // Fail2Ban Management
        .route("/api/security/fail2ban/unban", post(security::fail2ban_unban_ip))
        .route("/api/security/fail2ban/ban", post(security::fail2ban_ban_ip))
        .route("/api/security/fail2ban/{jail}/banned", get(security::fail2ban_banned))
        // Security Fix
        .route("/api/security/fix", post(security::apply_security_fix))
        // Login Audit
        .route("/api/security/login-audit", get(security::login_audit))
        // Panel Fail2Ban Jail
        .route("/api/security/panel-jail/setup", post(security::setup_panel_jail))
        .route("/api/security/panel-jail/status", get(security::panel_jail_status))
        // Security Compliance Report
        .route("/api/security/report", get(security::compliance_report))
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
        .route("/api/logs/stats", get(logs::log_stats))
        .route("/api/logs/docker", get(logs::docker_log_containers))
        .route("/api/logs/docker/{container}", get(logs::docker_log_view))
        .route("/api/logs/service/{service}", get(logs::service_logs))
        .route("/api/logs/sizes", get(logs::log_sizes))
        .route("/api/logs/truncate", post(logs::truncate_log))
        .route("/api/logs/check-errors", post(logs::check_errors))
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
        .route("/api/dns/propagation", post(dns::check_propagation))
        .route("/api/dns/health-check", post(dns::dns_health_check))
        .route("/api/dns/zones/{id}/dnssec", get(dns::dnssec_status))
        .route("/api/dns/zones/{id}/changelog", get(dns::dns_changelog))
        .route("/api/dns/zones/{id}/analytics", get(dns::dns_analytics))
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
        // Redirect Rules
        .route("/api/sites/{id}/redirects", get(sites::list_redirects).post(sites::add_redirect))
        .route("/api/sites/{id}/redirects/remove", post(sites::remove_redirect))
        // Password Protection
        .route("/api/sites/{id}/password-protect", get(sites::list_protected).post(sites::add_password_protect))
        .route("/api/sites/{id}/password-protect/remove", post(sites::remove_password_protect))
        // Domain Aliases
        .route("/api/sites/{id}/aliases", get(sites::list_aliases).post(sites::add_alias))
        .route("/api/sites/{id}/aliases/remove", post(sites::remove_alias))
        // Access Logs, Traffic Stats, PHP Errors, Health Check
        .route("/api/sites/{id}/access-logs", get(sites::access_logs))
        .route("/api/sites/{id}/stats", get(sites::site_stats))
        .route("/api/sites/{id}/php-errors", get(sites::php_errors))
        .route("/api/sites/{id}/health", get(sites::health_check))
        // Site Cloning
        .route("/api/sites/{id}/clone", post(sites::clone_site))
        // Custom SSL Upload
        .route("/api/sites/{id}/ssl/upload", post(sites::upload_ssl))
        // Environment Variables
        .route("/api/sites/{id}/env", get(sites::get_env_vars).put(sites::set_env_vars))
        // PHP Extensions Manager
        .route("/api/php/extensions/{version}", get(sites::php_extensions))
        .route("/api/php/extensions/install", post(sites::install_php_extension))
        // Agent endpoints (no cookie auth — uses Bearer token from servers table)
        .route("/api/agent/version", get(agent_updates::latest_version))
        .route("/api/agent/checkin", post(agent_checkin::checkin))
        .route("/api/agent/commands", get(agent_commands::poll))
        .route("/api/agent/commands/result", post(agent_commands::report_result))
        // API Keys
        .route("/api/api-keys", get(api_keys::list).post(api_keys::create))
        .route("/api/api-keys/{id}", delete(api_keys::revoke))
        .route("/api/api-keys/{id}/rotate", post(api_keys::rotate))
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
        // Live metrics WebSocket
        .route("/api/ws/metrics", get(ws_metrics::handler))
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
        .route("/api/services/install/powerdns", post(system::install_powerdns))
        .route("/api/services/install/{install_id}/log", get(system::install_log))
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
        // Mail: Rspamd spam filter
        .route("/api/mail/rspamd/install", post(mail::rspamd_install))
        .route("/api/mail/rspamd/status", get(mail::rspamd_status))
        .route("/api/mail/rspamd/toggle", post(mail::rspamd_toggle))
        // Mail: Webmail (Roundcube)
        .route("/api/mail/webmail/install", post(mail::webmail_install))
        .route("/api/mail/webmail/status", get(mail::webmail_status))
        .route("/api/mail/webmail/remove", post(mail::webmail_remove))
        // Mail: SMTP Relay
        .route("/api/mail/relay/configure", post(mail::relay_configure))
        .route("/api/mail/relay/status", get(mail::relay_status))
        .route("/api/mail/relay/remove", post(mail::relay_remove))
        // Mail: DNS Verification
        .route("/api/mail/domains/{id}/dns-check", get(mail::dns_check))
        // Mail: Logs & Storage
        .route("/api/mail/logs", get(mail::mail_logs))
        .route("/api/mail/storage", get(mail::mail_storage))
        // Mail: Blacklist/Reputation Check
        .route("/api/mail/blacklist-check", get(mail::blacklist_check))
        // Mail: Rate Limiting
        .route("/api/mail/rate-limit/set", post(mail::rate_limit_set))
        .route("/api/mail/rate-limit/status", get(mail::rate_limit_status))
        .route("/api/mail/rate-limit/remove", post(mail::rate_limit_remove))
        // Mail: Backup/Restore
        .route("/api/mail/backup", post(mail::mailbox_backup))
        .route("/api/mail/restore", post(mail::mailbox_restore))
        .route("/api/mail/backups", get(mail::mailbox_backups))
        .route("/api/mail/backups/delete", post(mail::mailbox_backup_delete))
        // Mail: TLS Enforcement
        .route("/api/mail/tls/status", get(mail::tls_status))
        .route("/api/mail/tls/enforce", post(mail::tls_enforce))
        // Agent Diagnostics proxy
        .route("/api/agent/diagnostics", get(system::diagnostics))
        .route("/api/agent/diagnostics/fix", post(system::diagnostics_fix))
        // System Logs (admin)
        .route("/api/system-logs", get(system_logs::list))
        .route("/api/system-logs/count", get(system_logs::count))
        // Activity (admin)
        .route("/api/activity", get(activity::list))
}
