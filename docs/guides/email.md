# Email Management Guide

## Prerequisites

Before setting up email, make sure:

- **Port 25 is open** -- Many cloud providers (AWS, GCP, Azure, Oracle) block outbound port 25 by default. You may need to request an unblock from your provider, or use SMTP relay (see below).
- **rDNS / PTR record is set** -- Your server's IP must have a reverse DNS record pointing to a hostname (e.g., `mail.example.com`). Set this in your VPS provider's control panel (Vultr, Hetzner, DigitalOcean, etc.), not in your domain DNS.
- **A domain with DNS access** -- You need to add MX, SPF, DKIM, and DMARC records.

Check if port 25 is open:

```bash
telnet smtp.gmail.com 25
```

If the connection times out, port 25 is blocked on your network.

## One-Click Install

DockPanel installs a complete mail server stack with one click.

1. Go to **Mail** in the sidebar
2. If the mail server is not installed, click **Install Mail Server**
3. DockPanel installs and configures:
   - **Postfix** -- SMTP server for sending and receiving email
   - **Dovecot** -- IMAP/POP3 server for reading email
   - **OpenDKIM** -- DKIM signing for email authentication

The installation takes about 30 seconds.

## Add a Mail Domain

1. Go to **Mail** > **Domains**
2. Click **Add Domain**
3. Enter your domain name: `example.com`
4. DockPanel generates the DKIM keys and shows the DNS records you need to add

## DNS Records

After adding a mail domain, you must add these DNS records at your domain registrar or DNS provider. DockPanel shows the exact values on the domain detail page.

### MX Record

Routes incoming email to your server:

```
Type: MX
Name: example.com (or @)
Value: mail.example.com
Priority: 10
TTL: 3600
```

Also add an A record for the mail subdomain:

```
Type: A
Name: mail
Value: 203.0.113.10  (your server IP)
TTL: 3600
```

### SPF Record

Tells receiving servers which IPs are allowed to send email for your domain:

```
Type: TXT
Name: example.com (or @)
Value: v=spf1 ip4:203.0.113.10 -all
TTL: 3600
```

Replace `203.0.113.10` with your server's public IP.

### DKIM Record

Cryptographic signature that proves emails were sent from your server. DockPanel generates a 2048-bit RSA key pair when you add the domain. Copy the value shown in the panel:

```
Type: TXT
Name: default._domainkey.example.com
Value: v=DKIM1; k=rsa; p=MIIBIjANBgkqh... (long key)
TTL: 3600
```

The full DKIM value is displayed in DockPanel's domain detail page. Copy it exactly.

### DMARC Record

Tells receiving servers what to do with emails that fail SPF or DKIM checks:

```
Type: TXT
Name: _dmarc.example.com
Value: v=DMARC1; p=quarantine; rua=mailto:postmaster@example.com
TTL: 3600
```

Start with `p=quarantine` (flag suspicious emails). Once you confirm everything works, change to `p=reject` (block spoofed emails).

## Create Mailboxes

1. Go to **Mail** > **Mailboxes**
2. Click **Add Mailbox**
3. Enter:
   - **Email address**: `user@example.com`
   - **Password**: A strong password
   - **Quota** (optional): Storage limit in MB
4. Click **Create**

You can also create:

- **Aliases** -- Forward `info@example.com` to `user@example.com`
- **Catch-all** -- Route all unmatched addresses to a single mailbox
- **Autoresponders** -- Out-of-office or auto-reply messages

## Test Sending and Receiving

### Test sending

Send a test email from the server:

```bash
echo "Test from DockPanel mail server" | mail -s "Test Email" recipient@gmail.com
```

Or use the mail queue viewer in the panel (Mail > Queue) to monitor outgoing messages.

### Test receiving

Send an email from an external account (Gmail, Outlook) to `user@example.com` and check:

1. The mailbox in the panel (Mail > Mailboxes > user@example.com)
2. Or connect with an IMAP client (Thunderbird, Outlook) using:
   - **IMAP server**: `mail.example.com`
   - **Port**: `993` (SSL/TLS)
   - **Username**: `user@example.com`
   - **Password**: The mailbox password

### Verify DNS records

Check your email authentication setup:

- **MX**: `dig MX example.com +short`
- **SPF**: `dig TXT example.com +short`
- **DKIM**: `dig TXT default._domainkey.example.com +short`
- **DMARC**: `dig TXT _dmarc.example.com +short`

Use [mail-tester.com](https://www.mail-tester.com) to check your overall email deliverability score.

## SMTP Relay

If your provider blocks port 25 (most cloud providers do), configure an SMTP relay to send email through a third-party service.

Supported relay providers: SendGrid, Mailgun, Amazon SES, Brevo, or any SMTP server.

1. Go to **Mail** > **Settings**
2. Enable **SMTP Relay**
3. Enter relay credentials:
   - **SMTP host**: `smtp.sendgrid.net`
   - **Port**: `587`
   - **Username**: `apikey`
   - **Password**: Your SendGrid API key
4. Save

DockPanel configures Postfix to route all outbound email through the relay. Incoming email still arrives directly to your server (port 25 inbound is not blocked by providers, only outbound).

## Webmail (Roundcube)

Roundcube provides browser-based email access.

1. Go to **Docker Apps**
2. Search for **Roundcube**
3. Click **Deploy**
4. Set a domain (e.g., `webmail.example.com`)

After deployment, users can access their email at `https://webmail.example.com`.

## Spam Filter (Rspamd)

Rspamd provides spam filtering, greylisting, and DKIM verification for incoming mail.

1. Go to **Docker Apps**
2. Search for **Rspamd**
3. Click **Deploy**

Rspamd includes a web interface for viewing spam statistics and adjusting filter rules.
