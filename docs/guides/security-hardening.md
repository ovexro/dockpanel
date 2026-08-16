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

1. Go to **My Account** (every role has it; administrators can also use **Settings** > **Account**)
2. Click **Enable 2FA**
3. Scan the QR code with your authenticator app
4. Enter the 6-digit code from the app to confirm
5. Save your **recovery codes** -- they are shown once and cannot be retrieved later

When 2FA is enabled, login requires your password followed by a TOTP code. The temporary token for the 2FA step expires after 5 minutes. Failed 2FA attempts are rate-limited to 5 per 5 minutes, on every route that checks a 2FA code -- signing in, disabling 2FA, and issuing new recovery codes.

> **Upgrading from a release before v2.84.0?** Two fixes in this area change what
> you should do. Enrolments made before **v2.83.0** never displayed their recovery
> codes (the panel generated them and stored them, but the block that draws them
> was never reached), so if you enrolled before then, **you are holding ten codes
> you have never seen**. Separately, codes issued before **v2.84.0** are half their
> current width. Either way the repair is the same: open **My Account** and click
> **New recovery codes** while you still have your authenticator. It does not
> disturb your enrolment. Until v2.84.0 the disable route was also not rate-limited.

### Recovery Codes

If you lose access to your authenticator app, use a recovery code -- to log in, and also to turn 2FA off or to issue a fresh set. Each code can only be used once, and you receive 10 when enabling 2FA.

**How many are left** is shown on the 2FA card, and the panel warns you when the set runs low or reaches zero. Before v2.84.0 the count was never surfaced anywhere, so an account could spend its last code without knowing.

**Issuing a new set.** Click **New recovery codes** on the 2FA card and confirm with a TOTP code or one of your remaining recovery codes. The previous set stops working immediately. Use this if you are running low, if you think a code has been seen by someone else, or if you enrolled before v2.83.0 and never received a readable set.

**How they are stored.** Recovery codes are hashed with SHA-256 before being written, so the plaintext is not in the database. SHA-256 is a *fast* hash, which means the width of the code is what actually protects it: from v2.84.0 codes are 16 hex characters (64 bits), which is not searchable. Codes issued **before** v2.84.0 were 8 characters (32 bits) -- small enough that anyone holding a copy of the `users` table, or an old backup, could recover them by exhaustive search. Those codes still work, and that is the second reason to reissue a set: it replaces short codes with long ones. Note that this is not the protection applied to your password, which is Argon2 with a per-user salt.

### Enforce 2FA

Admins can turn on the `enforce_2fa` setting. **This currently warns rather than refuses:** every user without 2FA sees a persistent banner asking them to enrol, and the login itself still succeeds. Refusing the login outright is deliberately not implemented, because several kinds of account have no second way in (OAuth-provisioned accounts with no password, the bootstrap administrator, non-browser callers), and a refusal they cannot satisfy is a lockout rather than a safeguard.

### Disable 2FA

Go to **My Account** > **Two-Factor Authentication** > **Disable 2FA**. Confirm with a TOTP code **or** one of your recovery codes -- accepting only a live TOTP code (the behaviour before v2.83.0) meant a lost authenticator could never be repaired by its owner.

### If a user has lost everything

An administrator can clear another user's 2FA from **Users** -- the shield icon on that user's row, which appears only for accounts that actually have an enrolment. It erases the enrolment and any remaining recovery codes, and signs out every session that user holds; they can then sign in with their password and enrol a new device. It is recorded in the activity log as `user.reset_2fa`.

Two limits worth knowing before you rely on it:

- **You cannot reset your own 2FA this way.** The route refuses it, because doing so would remove the factor without anyone presenting a code -- exactly what the disable flow exists to prevent. Use **My Account** with a TOTP or recovery code.
- **It does not remove passkeys**, which sign in on their own without the 2FA step. It is a repair for a locked-out account, not an eviction tool: if you are responding to a compromise, review that account's passkeys and sessions as well.
- **A sole administrator who has lost both their authenticator and their recovery codes cannot be recovered through the panel**, because there is no second administrator to perform the reset. Guard against this in advance: register a passkey (passkey sign-in does not require the 2FA step), keep a second administrator account, or keep your recovery codes somewhere you will still have them. Otherwise the only route back is clearing `totp_enabled`, `totp_secret` and `recovery_codes` for that row directly in PostgreSQL.

### Adding a passkey asks you to confirm who you are

Since v2.85.0, adding a passkey requires you to re-present a credential you already hold — your current password, a code from your authenticator, or one of your recovery codes. **Any one of them is enough**, deliberately: an administrator who has lost their authenticator still knows their password, and the bullet above tells that person to register a passkey. A door that demanded a TOTP code would refuse them at exactly the moment they need it.

The reason for the prompt is that a passkey is the only credential this panel mints that **survives every reset it offers**. Clearing a user's 2FA, resetting their password and signing out all of their sessions leave an enrolled passkey working, and passkey sign-in does not take the 2FA step. So if someone reaches an open session — a shared machine, a stolen cookie — enrolling a passkey would convert that moment into permanent access. Confirming a credential closes that, because a session hijacker holds the session and nothing else.

Two consequences worth knowing:

- **An account that signs in only through an identity provider** (Google, GitHub) has no password to confirm and may have no authenticator. It is asked to sign in again with its provider if its session is more than five minutes old, and is not prompted at all if it has just signed in. This is a weaker check than a password and is not equivalent to one; it bounds a session replayed from another machine, not a script running on the panel's own origin.
- **You are notified when a passkey is added to your account**, in the panel's notification centre, and passkeys now appear in **Export my data**. If a passkey you do not recognise appears, remove it in **My Account** and change your password.

Removing a passkey is not guarded, and deliberately so: the person who has lost the authenticator holding it is exactly the person who needs to remove it, and a passkey is never an account's only way in — the password and identity-provider doors remain.

## IP Whitelist

Restrict panel access to specific IP addresses. When configured, login attempts from non-whitelisted IPs are rejected before password validation.

The allowlist covers **every way of signing in** — password, passkey, the OAuth callback, and the second step of a 2FA login. Before v2.47.0 it guarded the password form alone, so an operator who restricted the panel by IP still had the passkey and SSO paths answering from anywhere. If you set an allowlist on an earlier release, re-check it after upgrading: it is now enforced on doors that previously ignored it.

Set **Panel IP Allowlist** in **Settings** > **Account** > **Security Hardening** to a comma-separated list of IPs or CIDR ranges (`203.0.113.4, 10.0.0.0/8, 2001:db8::/32`). Leave it empty to allow all IPs. Entries are validated on save, so a malformed range is rejected rather than stored.

The check reads the client address from the `X-Real-IP` header. **If your reverse proxy does not set it, a non-empty allowlist rejects every login**, including yours — the check fails closed on purpose, since an allowlist that cannot identify the caller must not admit them. Confirm logins work from an allowlisted address before you rely on it. To recover from a lockout, clear the value directly in the database:

```sql
DELETE FROM settings WHERE key = 'allowed_panel_ips';
```

## Locked out by "Email not verified"

Sign-in is refused with **"Email not verified. Check your inbox."** when an account is
unverified *and* an SMTP host is set in Settings. The check is on the setting being
**present**, not on the mail actually working — so a half-finished or wrong SMTP
configuration arms the gate just as effectively as a working one.

Before v2.90.0 this could lock out the very first administrator. The account created by
the initial setup screen was stored unverified and, because it never registered, it
never held a verification token — so the verification link could not be resent, it could
not be generated at all, and the password-reset route needed the same broken mail. From
v2.90.0 the setup screen marks that account verified when it creates it, and upgrading
also releases any existing account that is unverified and holds no token.

To clear it on a running panel, as an administrator: **Security** > **Approvals**, find
the account under *Accounts blocked by email verification*, and press **Mark verified**.
Rows marked `no link exists` cannot verify themselves by any means; rows marked
`link outstanding` still have a live link that a working SMTP would deliver.

If no administrator can sign in at all, clear it directly in the database:

```sql
UPDATE users SET email_verified = TRUE WHERE email = 'you@example.com';
```

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
| `POST` | `/api/auth/2fa/disable` | Disable 2FA (TOTP or recovery code) |
| `POST` | `/api/auth/2fa/recovery-codes` | Issue a fresh set of recovery codes |
| `GET` | `/api/auth/2fa/status` | 2FA state, enforcement flag, recovery codes remaining |
| `POST` | `/api/users/{id}/reset-2fa` | Admin: clear another user's 2FA enrolment |
| `GET` | `/api/auth/export-my-data` | GDPR data export |
| `POST` | `/api/security/ssh/disable-password` | Disable SSH password auth |
| `POST` | `/api/security/ssh/enable-password` | Enable SSH password auth |
| `POST` | `/api/security/ssh/disable-root` | Disable SSH root login |
| `POST` | `/api/security/ssh/change-port` | Change SSH port |
| `GET` | `/api/security/lockdown` | Get lockdown status |
| `POST` | `/api/security/lockdown/activate` | Activate system lockdown |
| `POST` | `/api/security/lockdown/deactivate` | Deactivate lockdown |
| `POST` | `/api/security/panic` | Emergency panic button |
| `POST` | `/api/security/forensic-snapshot` | Capture forensic system state |
| `GET` | `/api/security/audit-log` | Query immutable audit log |
| `GET` | `/api/security/recordings` | List terminal recordings |
| `GET` | `/api/security/pending-users` | List users awaiting approval |
| `POST` | `/api/security/users/{id}/approve` | Approve a pending user |

## Advanced Security Features

### System Lockdown

Lockdown mode blocks all non-admin access. When active:
- Terminal sessions are disabled
- Registration is blocked
- Non-admin users cannot login

Activate from **Security** > **Lockdown** tab, or via the API. Lockdown auto-expires after 24 hours.

### Panic Button

The panic button performs an emergency lockdown: kills all active terminal sessions, blocks non-admin access, and disables registration. Available in **Security** > **Lockdown** tab.

### Immutable Audit Log

All security events are written to the `security_audit_log` table, and a PostgreSQL trigger rejects `UPDATE` and `DELETE` on it. That trigger is the guarantee — it is what makes the record in the database immutable, and it is what the panel reads.

Every event is **also** appended to a dated file on disk at `/var/lib/dockpanel/audit/`, as a convenience copy for host-level forensics and log shipping. **What this does not do, stated plainly:** those files are written in append mode, but nothing sets a kernel append-only attribute on them, so a process that can write the directory can still rewrite one in place. Treat the on-disk copies as a convenience, not as evidence — the database is the authoritative record. Nothing in DockPanel reads these files back; they are for you and your own tooling.

If you want kernel-enforced protection on the on-disk copies, apply it to the **files**, not the directory:

```bash
chattr +a /var/lib/dockpanel/audit/*.log     # new files each day need it again
```

Setting it on the directory instead is a common mistake and does not do what it looks like: it blocks deleting and renaming the logs while still allowing any of them to be truncated and rewritten. It also blocks DockPanel's own 365-day retention sweep, which cannot unlink under that attribute even as root — the panel logs a warning naming the directory when that happens. Either attribute can be cleared by root (`chattr -a`), so neither defends against an attacker who already has it.

View the log in **Security** > **Lockdown** tab (Audit Log section).

### Terminal Session Recording

Terminal sessions are recorded in asciicast v2 format while **Terminal Session Recording** is on in **Settings** > **Account** > **Security Hardening**. Recordings are stored at `/var/lib/dockpanel/recordings/` and listed in **Security** > **Recordings** tab. Retention: 30 days.

The panel decides per session and tells the agent inside the signed connection ticket, so a user cannot opt their own session out. Turning the toggle off stops new recordings; it does not delete existing ones. Fleet members running an agent older than v2.46.0 ignore the setting and keep recording — update the agent for the toggle to take effect there.

### Geo-IP Login Alerts

When enabled, DockPanel alerts admins when a login or registration occurs from a new IP address, especially from VPN, proxy, or datacenter IPs. Configure in **Settings** > **Account** > **Security Hardening** section.

### Registration Approval Mode

When enabled, new user registrations require admin approval before the user can login. Pending users appear in **Security** > **Approvals** tab. Enable in **Settings** > **Account** > **Security Hardening**.

### Auto-Lockdown

If the system detects a configurable number of suspicious events within a time window (default: 5 events in 10 minutes), lockdown activates automatically. Configure **both** halves — **Auto-Lockdown Threshold** and **Auto-Lockdown Window** — in **Settings** > **Account** > **Security Hardening**.

Lockdown blocks non-admin access until it expires (24 hours) or an admin deactivates it from **Security** > **Lockdown**. Note that the default threshold is low and a single burst can reach it, so raise the threshold or widen the window if your panel sees legitimate bursts of flagged activity.

Admins can always sign in, so **Security** > **Lockdown** is the normal way out. If you cannot reach the panel at all, clear it directly in the database:

```sql
UPDATE lockdown_state SET active = false, triggered_by = NULL, triggered_at = NULL, reason = NULL WHERE id = 1;
```

To also drop the events that tripped it, so the next check does not immediately re-trigger:

```sql
DELETE FROM suspicious_events WHERE created_at > NOW() - INTERVAL '10 minutes';
```

> **Changed in 2.46.0.** Suspicious-event counting, lockdown expiry and canary monitoring used to be gated by the **auto-healing** switch, so turning auto-healing off silently disabled all three. They now run independently. Auto-healing is **off by default**, so this affects any install that never switched it on — not only those that deliberately turned it off. Configure the threshold deliberately rather than relying on that side effect.
>
> **Fixed in 2.47.0.** While counting was gated off, the agent still queued suspicious events to disk and nothing drained the file. On 2.46.0 the first check after upgrading read that whole backlog and counted it as having happened at that moment, so a box that had ever seen five suspicious commands locked down immediately — for 24 hours, blocking every non-admin user. 2.47.0 records each event at the time it actually occurred, so an old backlog falls outside the window. If 2.46.0 locked you out, clear it with the SQL above and upgrade.

### Site Creation Rate Limit

One user may create at most **Site Creation Rate Limit** sites per hour (default 3); set it to 0 to remove the limit. Hitting it records a suspicious event, which feeds auto-lockdown. Configure it in **Settings** > **Account** > **Security Hardening**.

### Canary Files

While **Canary File Monitoring** is on, DockPanel checks hidden canary files in sensitive directories (`/etc/`, `/root/`, `/home/`, `/var/www/`) every 2 minutes and alerts if their access times change. Canary monitoring, suspicious-event ingestion and auto-lockdown expiry run independently of the auto-healer's own switch.

### Backup Integrity Hashes

Backups taken through the panel record a SHA-256 hash of the archive and the hash of the previous backup of the same resource. Both appear in the chain-of-trust report.

**What this does not do, stated plainly:** nothing re-computes those hashes afterwards, so a tampered archive is not detected and no alert fires. The hashes are a record of what was written at the time — useful for comparing against a copy you verify yourself — not tamper detection.

**Every path records one as of v2.118.0.** Until then the two unattended *site* paths — a per-site schedule and a backup policy — did not, while manual site backups and every database and volume backup did. Backups those two paths took before v2.118.0 keep no hash; nothing back-fills one, because the archive's hash has to be taken when the archive is written. A report for a backup with no hash shows `—` rather than a value.

### Suspicious Command Detection

Terminal commands matching dangerous patterns (useradd, chpasswd, su, curl|bash, etc.) are flagged and reported to the admin. These events feed into the auto-lockdown threshold.

### Panel Database Auto-Backup

DockPanel's own PostgreSQL database is automatically backed up daily to `/var/backups/dockpanel/` with 7-day retention. Enable/disable in **Settings** > **Account** > **Security Hardening**.
