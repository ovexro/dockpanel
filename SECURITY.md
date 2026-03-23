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
- **Input validation** — All user-supplied data is validated and sanitized before processing.
- **Systemd hardening** — Generated service units apply systemd security directives to limit the blast radius of any compromise.
- **Terminal sandboxing** — Terminal sessions run with `PR_SET_NO_NEW_PRIVS`, restricted bash shells, and a command blocklist to prevent privilege escalation and dangerous operations.

## Past Security Work

A comprehensive security audit was completed in March 2026. The audit identified and resolved **117 vulnerabilities** across **45 files**, spanning the following categories:

- Command injection
- Path traversal
- Configuration injection
- Missing authentication and authorization checks
- Privilege escalation
- Input validation gaps

All identified issues have been fixed. The audit informed many of the architectural decisions described above.

## Contact

For security-related inquiries, reach us at **security@dockpanel.dev**.
