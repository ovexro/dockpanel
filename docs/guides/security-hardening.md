# Security Hardening

## Security Scanner

DockPanel runs a full security scan automatically every 7 days. You can also trigger a scan manually from **Security** > **Run Scan** or via the API.

The scanner checks the server through the agent and reports findings in three severity levels:

- **Critical** -- Immediate action required (e.g., world-writable config files, exposed credentials)
- **Warning** -- Should be fixed (e.g., SSH password auth enabled, missing firewall)
- **Info** -- Informational (e.g., SSH on default port, non-critical suggestions)

Each finding includes a title, description, affected file path (if applicable), and a remediation suggestion.

### File Integrity Monitoring

During each scan, the agent computes SHA-256 hashes of critical system files. These hashes are stored as baselines in the `file_integrity_baselines` table. On subsequent scans, if a file's hash has changed, a security finding is created to flag the modification. This detects unauthorized changes to system binaries, config files, or web application code.

## Security Score

The security score is calculated as:

```
Score = 100 - (critical_findings * 20) - (warning_findings * 5)
```

A score of 100 means no findings. The score is shown on the Security page and in the downloadable compliance report.

## Firewall (UFW)

DockPanel manages the server firewall through UFW.

### View firewall status

Go to **Security** > **Firewall** to see all rules and whether UFW is active.

### Add a rule

1. Go to **Security** > **Firewall**
2. Click **Add Rule**
3. Enter:
   - **Port**: The port number (1-65535)
   - **Protocol**: `tcp`, `udp`, or `tcp/udp`
   - **Action**: `allow`, `deny`, or `reject`
   - **From** (optional): Restrict to a specific IP or CIDR range
4. Click **Add**

When you create a site, DockPanel automatically configures firewall rules for ports 80 and 443. Docker container proxy ports are blocked from external access by default.

### Delete a rule

Click the delete icon next to any rule in the list, or use the API:

```bash
curl -X DELETE https://panel.example.com/api/security/firewall/rules/RULE_NUMBER \
  -H "Cookie: dp_token=YOUR_TOKEN"
```

### From the CLI

```bash
dockpanel security firewall list
dockpanel security firewall allow 8080/tcp
dockpanel security firewall deny 3306/tcp from 0.0.0.0/0
```

## Fail2Ban

Fail2Ban monitors log files for repeated authentication failures and bans offending IPs.

### Status

Go to **Security** > **Fail2Ban** to see running jails, banned IPs, and ban counts.

### Panel Login Jail

DockPanel can create a dedicated Fail2Ban jail that monitors the panel's own login endpoint. Set it up from **Security** > **Panel Jail** > **Setup**.

### Manual Ban / Unban

From the panel or API, you can manually ban or unban an IP in any jail:

```bash
# Ban an IP
curl -X POST https://panel.example.com/api/security/fail2ban/ban \
  -H "Cookie: dp_token=YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"ip": "1.2.3.4", "jail": "sshd"}'

# Unban an IP
curl -X POST https://panel.example.com/api/security/fail2ban/unban \
  -H "Cookie: dp_token=YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"ip": "1.2.3.4", "jail": "sshd"}'
```

### List Banned IPs

```bash
curl -s https://panel.example.com/api/security/fail2ban/sshd/banned \
  -H "Cookie: dp_token=YOUR_TOKEN"
```

## Two-Factor Authentication (2FA)

DockPanel supports TOTP-based 2FA using any authenticator app (Google Authenticator, Authy, 1Password, etc.).

### Enable 2FA

1. Go to **Settings** > **Security**
2. Click **Enable 2FA**
3. Scan the QR code with your authenticator app
4. Enter the 6-digit code from the app to confirm
5. Save your **recovery codes** -- these are shown once and cannot be retrieved later

When 2FA is enabled, login requires your password followed by a TOTP code. The temporary token for the 2FA step expires after 5 minutes. Failed 2FA attempts are rate-limited to 5 per 5 minutes.

### Recovery Codes

If you lose access to your authenticator app, use a recovery code to log in. Each code can only be used once. You receive 10 codes when enabling 2FA. Recovery codes are stored as Argon2 hashes -- they cannot be retrieved from the database.

### Enforce 2FA

Admins can enforce 2FA for all users by enabling the `enforce_2fa` setting. Users without 2FA will be prompted to set it up on their next login.

### Disable 2FA

Go to **Settings** > **Security** > **Disable 2FA**. You must enter a valid TOTP code to confirm.

## IP Whitelist

Restrict panel access to specific IP addresses. When configured, login attempts from non-whitelisted IPs are rejected before password validation.

Set the `allowed_panel_ips` setting in **Settings** with a comma-separated list of IPs or CIDR ranges. Leave empty to allow all IPs.

## SSH Hardening

From **Security**, you can apply SSH hardening with one click:

- **Disable password authentication** -- Force key-based login only
- **Disable root login** -- Prevent direct root SSH access
- **Change SSH port** -- Move SSH to a non-standard port

Each action is logged in the activity log. Ensure you have an SSH key configured before disabling password auth, or you will be locked out.

## Login Audit

**Security** > **Login Audit** shows recent login attempts for both the panel and SSH:

- **Panel logins**: Successful and failed attempts with IP, timestamp, and user agent
- **SSH logins**: Parsed from `auth.log` on the server by the agent

## Auto-Fix

The security scanner identifies findings that can be fixed automatically. Click **Fix** next to any auto-fixable finding to apply the remediation. Examples include:

- Renewing an expiring SSL certificate
- Fixing file permissions on config files
- Disabling debug mode in web applications

Each fix is logged in the activity log with the fix type and target.

## Compliance Report

Go to **Security** > **Download Report** to generate an HTML compliance report. The report includes:

- Security score with color-coded rating
- Infrastructure status (firewall, Fail2Ban, SSH configuration, SSL certificates)
- Scan summary (total, critical, warning findings)
- Detailed findings table with severity, description, and remediation steps

The report is styled for printing and can be shared with auditors.

## GDPR Data Export

Users can export all their personal data stored in DockPanel:

```bash
curl -s https://panel.example.com/api/auth/export-my-data \
  -H "Cookie: dp_token=YOUR_TOKEN" | jq
```

The export includes account details (email, role, 2FA status), site list, recent activity log entries, and active sessions with IP addresses.

## Session Management

See the [Session Management guide](sessions.md) for details on viewing, revoking, and managing active sessions.

## API Reference

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/security/overview` | Security overview (firewall, Fail2Ban, SSH status) |
| `GET` | `/api/security/firewall` | Firewall status and rules |
| `POST` | `/api/security/firewall/rules` | Add a firewall rule |
| `DELETE` | `/api/security/firewall/rules/{number}` | Delete a firewall rule |
| `GET` | `/api/security/fail2ban` | Fail2Ban status |
| `GET` | `/api/security/fail2ban/{jail}/banned` | List banned IPs for a jail |
| `POST` | `/api/security/fail2ban/ban` | Manually ban an IP |
| `POST` | `/api/security/fail2ban/unban` | Unban an IP |
| `POST` | `/api/security/panel-jail/setup` | Create the panel login jail |
| `GET` | `/api/security/panel-jail/status` | Check panel jail status |
| `POST` | `/api/security/scan` | Trigger a security scan (admin) |
| `GET` | `/api/security/scans` | List past scans |
| `GET` | `/api/security/scans/{id}/findings` | Get findings for a scan |
| `POST` | `/api/security/fix` | Apply a security fix |
| `GET` | `/api/security/report` | Download HTML compliance report |
| `GET` | `/api/security/login-audit` | Recent login attempts |
| `POST` | `/api/auth/2fa/setup` | Generate TOTP secret and QR code |
| `POST` | `/api/auth/2fa/enable` | Verify code and enable 2FA |
| `POST` | `/api/auth/2fa/verify` | Complete login with TOTP code |
| `POST` | `/api/auth/2fa/disable` | Disable 2FA |
| `GET` | `/api/auth/2fa/status` | Check if 2FA is enabled |
| `GET` | `/api/auth/export-my-data` | GDPR data export |
| `POST` | `/api/security/ssh/disable-password` | Disable SSH password auth |
| `POST` | `/api/security/ssh/enable-password` | Enable SSH password auth |
| `POST` | `/api/security/ssh/disable-root` | Disable SSH root login |
| `POST` | `/api/security/ssh/change-port` | Change SSH port |
