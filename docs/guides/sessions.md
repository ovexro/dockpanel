# Sessions Guide

DockPanel tracks all active login sessions and provides session management, revocation, and GDPR data export.

## View Active Sessions

1. Go to **Settings** > **Sessions** (or click your profile avatar > **Sessions**)
2. View all active sessions with:
   - **Device**: Browser and OS
   - **IP Address**: Source IP
   - **Location**: Approximate location (if available)
   - **Last Active**: When the session was last used
   - **Current**: Badge indicating which session you are using now

## Revoke a Session

To log out a specific session (e.g., a device you no longer use):

1. Find the session in the list
2. Click **Revoke**
3. The session is immediately invalidated

The user on that device will be redirected to the login page on their next request.

### Revoke All Sessions

To log out everywhere except your current session:

1. Go to **Settings** > **Sessions**
2. Click **Revoke All Other Sessions**

This is useful if you suspect your account has been compromised.

## GDPR Data Export

Download all personal data associated with your account:

1. Go to **Settings** > **Sessions**
2. Click **Export My Data**
3. A JSON file downloads containing:
   - Account profile
   - Login history
   - Session history
   - Activity log
   - Notification preferences

This complies with GDPR Article 20 (right to data portability).

## Session Security

- **JWT expiry**: Tokens expire after 2 hours
- **Session binding**: Sessions are bound to the originating IP (configurable)
- **Concurrent limits**: Admins can set maximum concurrent sessions per user in **Settings** > **Security**

## API Reference

See the [Sessions API](../api-reference.md#sessions) for all endpoints.
