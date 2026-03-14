# Changelog

All notable changes to DockPanel will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [1.0.0] - 2026-03-14

### Added
- **Core Panel**: Site management (static, PHP, proxy), database management (PostgreSQL, MariaDB), SSL (Let's Encrypt), file manager, web terminal, backups
- **Docker Apps**: 34 one-click templates across 10 categories + Docker Compose import
- **CLI**: Full command-line interface — status, sites, db, apps, ssl, backup, logs, security, diagnose, export, apply
- **Infrastructure as Code**: YAML export/import of server configuration
- **Smart Diagnostics**: Pattern-based issue detection across 6 categories with one-click fixes
- **Auto-Healing**: Automatic restart of crashed services, log cleanup on full disk, SSL renewal
- **Alerting System**: 5 alert types (CPU/memory/disk thresholds, server offline, SSL expiry, service health, backup failure) with email, Slack, Discord notifications
- **2FA/TOTP**: Full two-factor authentication with QR setup and recovery codes
- **Dashboard Intelligence**: Health score (0-100), top active issues, SSL expiry countdowns
- **Docker Resource Limits**: Memory and CPU limits on container deploy
- **Container Management**: Health checks, logs viewer, environment viewer, one-click updates
- **Security**: Firewall management, Fail2Ban, SSH hardening, security scanning with scoring
- **DNS Management**: Cloudflare DNS zone management with full record CRUD
- **Git Deploy**: Webhook-triggered deployments from Git repos
- **Staging Environments**: Create staging copies, sync from production, push to live
- **Uptime Monitoring**: HTTP checks with configurable intervals and incident tracking
- **Teams**: Multi-user access with roles and team-based permissions
- **Activity Log**: Full audit trail of all admin actions
- **Multi-Server**: Manage unlimited servers from a single dashboard
- **ARM64 Support**: Pre-built binaries for Raspberry Pi and ARM64 servers
- **Auto Reverse Proxy**: Domain + SSL auto-configured when deploying Docker apps
- **Command Palette**: Ctrl+K global search across all panel pages
- **Notification Channels**: Email toggle, Slack/Discord webhook configuration
- **Custom Nginx Directives**: Per-site textarea for advanced nginx config
- **Onboarding Wizard**: 5-step getting started checklist for new users

### Security
- JWT auth with HttpOnly cookies + Bearer header support
- Token blacklist for logout with periodic cleanup
- Argon2 password hashing
- Rate limiting on login, 2FA, webhooks, and agent endpoints
- Systemd hardening (NoNewPrivileges, ProtectSystem, MemoryMax)
- Nginx rate limiting (30r/s on API)
- 12 CHECK constraints on database status/type fields
- Atomic nginx config writes (tmp+rename)

### Infrastructure
- Supervised background tasks with auto-restart on panic
- Statement timeout on all database pool connections (30s)
- Agent request timeout (60s)
- DB backup cron (daily, 7-day retention)
- Docker prune cron (weekly)
