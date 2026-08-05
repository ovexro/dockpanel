# Sessions Guide

DockPanel tracks active login sessions and provides per-session revocation and a
GDPR data export.

## View Active Sessions

1. Go to **Settings** > **Sessions**
2. Each session shows:
   - **IP Address**: the source IP recorded at login, or `unknown` when the
     request carried no forwarded address (a loopback call from a deploy script,
     for example)
   - **User agent**: the raw client string the browser or tool sent
   - **Last seen**: when the session was last used
   - **This device**: a badge on the session you are currently using

There is no location column. DockPanel does not geolocate sessions and stores no
country, city or coordinates.

## Revoke a Session

To log out a specific session — a device you no longer use, or one you do not
recognise:

1. Find the session in the list
2. Click **Revoke**
3. The session is invalidated immediately: its token is blacklisted, in memory
   and in the database, so it does not come back after a restart

The client on that device is sent to the login page on its next request. You
cannot revoke the session you are currently using; log out instead.

### Revoke All Sessions — read this before pressing it

**This button is panel-wide, not account-wide.** It logs out *every user of this
panel*, including you. It is an operator control for a suspected compromise of
the panel itself, not a way to tidy up your own devices — and it is **admin
only**, so a non-admin will get a permission error.

To sign your own other devices out, revoke them individually from the list
above. That is the account-level control, and it is available to every user.

## GDPR Data Export

Download the personal data associated with your account:

1. Go to **Settings** > **Sessions**
2. Click **Export My Data**
3. A JSON file downloads containing:
   - Account profile — email, role, OAuth provider, whether 2FA is on, created date
   - Your sites — domain and runtime
   - Recent activity — the last 100 actions
   - Session history — IP, user agent and creation time

This supports GDPR Article 20 (right to data portability) for the data listed
above. It is a portability export, not a full disclosure of every record the
panel holds that mentions you: server-side logs, backups and audit entries are
retained separately under their own retention settings.

## Session Security

- **JWT expiry**: tokens expire after 2 hours
- **Revocation survives restart**: revoked token IDs are persisted, not just held
  in memory
- **Suspension**: suspending an account revokes its sessions at the same time

DockPanel does **not** bind a session to its originating IP, and does not limit
the number of concurrent sessions per user. Earlier versions of this guide said
it did; no such setting has ever existed.

## API Reference

See the [Sessions API](../api-reference.md#sessions) for all endpoints.
