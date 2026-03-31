# Security Policy

## Supported Versions

Only the latest release of DockPanel is supported with security updates. We recommend always running the most recent version to ensure you have the latest fixes and protections.

| Version        | Supported |
| -------------- | --------- |
| Latest release | Yes       |
| Older releases | No        |

## Reporting a Vulnerability

If you discover a security vulnerability in DockPanel, please report it responsibly. **Do not open a public GitHub issue for security vulnerabilities.**

### How to Report

Send an email to **security@dockpanel.dev** with the following information:

- A clear description of the vulnerability
- Step-by-step reproduction instructions
- An assessment of the impact (what an attacker could achieve)
- Any relevant logs, screenshots, or proof-of-concept code
- Your preferred name or handle for credit (if you would like to be acknowledged)

### Response Timeline

- **48 hours** — We will acknowledge receipt of your report
- **7 days** — We will provide an initial assessment and severity classification
- We will keep you informed of our progress toward a fix

## Responsible Disclosure Policy

We ask security researchers to follow responsible disclosure practices:

- **Please give us 90 days** from the initial report to develop and release a fix before any public disclosure.
- We will **credit researchers** who follow responsible disclosure, with their permission.
- We **do not pursue legal action** against security researchers acting in good faith. Good-faith research means you made a genuine effort to avoid privacy violations, data destruction, and service disruption.

We appreciate the work of security researchers and are committed to working with the community to keep DockPanel safe.

## Scope

### In Scope

The following components are covered by this security policy:

- DockPanel agent
- API and backend services
- Command-line interface (CLI)
- Frontend web application
- Install and setup scripts

### Out of Scope

The following are not covered by this policy:

- **Third-party dependencies** — Please report these to the upstream project directly
- **Social engineering attacks** (e.g., phishing DockPanel users or maintainers)
- **Denial of service (DoS) attacks**
- Vulnerabilities in infrastructure not maintained by the DockPanel project

## Security Architecture Summary

DockPanel is designed with defense in depth. Key security properties include:

- **Unix socket communication** — The agent communicates via a Unix domain socket and is not exposed to the network.
- **JWT authentication** — All API endpoints require valid JWT tokens for access.
- **Argon2 password hashing** — User passwords are hashed using Argon2, a memory-hard algorithm resistant to brute-force and GPU-based attacks.
- **Credential encryption at rest** — All stored credentials (DB passwords, SMTP, S3/SFTP, OAuth, TOTP, DKIM) are encrypted with AES-256-GCM using dedicated key derivation.
- **Content Security Policy** — CSP headers are set on the frontend nginx configuration to mitigate XSS and data injection attacks.
- **Safe command execution** — All child processes are spawned with `env_clear()` to prevent LD_PRELOAD, PATH hijacking, and other environment-based attacks.
- **Rate limiting** — All authentication endpoints are rate-limited to prevent brute-force attacks.
- **IDOR protection** — All resource endpoints verify ownership before granting access.
- **Input sanitization** — All user-supplied data is validated and sanitized before being passed to system commands.
- **Systemd hardening** — Generated service units apply systemd security directives to limit the blast radius of any compromise.
- **Terminal sandboxing** — Terminal sessions run with `PR_SET_NO_NEW_PRIVS`, restricted bash shells, and a command blocklist to prevent privilege escalation and dangerous operations.

## Past Security Work

### Audit Round 6: Fresh Zero-Assumptions Audit (March 2026)

A complete from-scratch security audit with six parallel agents treating the codebase as entirely unknown. This covered all 222 Rust files and 506 TypeScript files with zero prior assumptions. **30 findings** fixed across 24 files:

- **MySQL SQL injection** — Parameterized all dynamic queries.
- **Deploy script RCE** — Sanitized user-controlled deploy commands.
- **CSRF protection** — Added `X-Requested-With` header enforcement.
- **Compose YAML validation** — Rewrote from string matching to `serde_yaml_ng` AST parsing.
- **KDF upgrade** — SHA-256 replaced with HKDF (backwards-compatible legacy fallback).
- **Agent TLS default** — Changed from insecure to strict by default.
- **Terminal filename injection**, **Laravel command injection**, **shell blocklist hardening**, **cron filter gaps**, **WP plugin slug validation**.
- **Stripe timing attack**, **symlink attack**, **mail injection**, **SMTP CRLF**, **dashboard cross-user leak**, **backup path traversal**, **migration container validation**, **stack template passwords randomized**, **socket permissions tightened**, **env leak in Command::new**.

### Audit Rounds 4-5: Feature Gap Audit + Error Handling Hardening (March 2026)

Audited all agent and backend code for silent error suppression and missing functionality:

- **59 silent `.ok()` failures** in the agent replaced with proper error handling and logging.
- **51 `.ok().flatten()` anti-patterns** in the backend replaced with error propagation.
- **45+ command timeouts** added to agent (Docker, systemctl, apt, system commands) to prevent hanging.
- **Uninstall routes** added for all 10 services (PHP, Certbot, UFW, Fail2Ban, PowerDNS, Redis, Node.js, Composer, mail server, PHP versions).
- **SSL certificate management** — force-renewal and deletion endpoints added.
- **User lifecycle** — suspend/unsuspend with session invalidation, admin password reset.
- **Installer hardening** — silent package failures now warn, Docker volume cleanup prevents DB password mismatch on retry.

### Audit Round 3: Research-Driven Audit (March 2026)

A research-driven security audit studied real-world CVEs from CyberPanel, HestiaCP, CloudPanel, VestaCP, Webmin, and cPanel, then audited DockPanel against those attack patterns. This round identified **55 findings** (12 HIGH, 28 MEDIUM, 15 LOW), including:

- **Command execution safety** — Added `safe_command()` with `env_clear()` on all 341 `Command::new()` calls across 44 files to prevent LD_PRELOAD/PATH hijacking.
- **Credential encryption at rest** — All stored credentials encrypted with AES-256-GCM using dedicated key derivation.
- **Shell injection** — Rewrote database_backup.rs to pipe `docker exec` + `gzip` instead of `bash -c` with interpolated strings.
- **Tar symlink attacks** — `--no-dereference` on backup creation, `--no-same-owner` on restore.
- **Deploy log IDOR** — Ownership verification on SSE streams.
- **Content Security Policy** — CSP header added to frontend nginx config.
- **Docker exec denylist** — 7 escape-relevant commands blocked (unshare, pivot_root, setns, capsh, mknod, debugfs, kexec).
- **WebSocket security** — Conditional upgrade to prevent h2c smuggling, `access_log off` on token-bearing WS locations.

### Audit Rounds 1-2: Comprehensive Audit (March 2026)

The initial comprehensive security audit identified and resolved **117 vulnerabilities** across **45 files**, spanning the following categories:

- Command injection
- Path traversal
- Configuration injection
- Missing authentication and authorization checks
- Privilege escalation
- Input validation gaps

All identified issues across all six audit rounds have been fixed. Combined total: **260+ vulnerabilities** found and resolved.

## Contact

For security-related inquiries, reach us at **security@dockpanel.dev**.
