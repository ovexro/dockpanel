# Getting Started

## What is DockPanel?

DockPanel is a free, self-hosted, Docker-native server management panel built in Rust. It lets you manage sites, databases, Docker apps, SSL certificates, backups, email, DNS, and security from a single web interface or CLI. It installs in under 60 seconds, the panel services themselves idle at about ~49MB of RAM (about ~109MB total with the bundled PostgreSQL), and it runs on x86_64 and ARM64 servers with no subscriptions or artificial limits.

## System Requirements

| Requirement | Minimum | Recommended |
|-------------|---------|-------------|
| **OS** | Ubuntu 20.04+, Debian 11+, CentOS 9+, Rocky Linux 9+, AlmaLinux 9+, Fedora 39+ | Ubuntu 22.04 LTS |
| **Architecture** | x86_64 or ARM64 (aarch64) | x86_64 |
| **RAM** | 512 MB | 1 GB+ |
| **Disk** | 10 GB | 20 GB+ |
| **CPU** | 1 core | 2 cores |

Docker and Nginx are installed automatically if not already present.

> **On the RHEL family (CentOS, Rocky, AlmaLinux, Fedora)** the panel, sites, PHP, SSL,
> Docker apps and the **optional-service installers** all work, and the installer configures
> firewalld and SELinux for you. Redis, Node.js, PowerDNS, the WAF, Cloudflare Tunnel,
> Composer, Fail2Ban and PHP extensions were each installed from the panel on Rocky 9.8.
> Two deliberate exceptions remain: the **mail server** still refuses on RPM (the packages
> resolve, but its configuration layout has not been driven there yet), and **UFW** refuses
> on any box already running firewalld — installing it would create a second rule set that
> nothing consults, so the panel opens ports through firewalld instead. Both say so plainly
> rather than failing obscurely.

## Installation

Run a single command on a fresh VPS:

```bash
curl -sL https://dockpanel.dev/install.sh | sudo bash
```

The installer will:

1. Detect your OS and package manager
2. Install Docker, Nginx, PHP-FPM, Certbot, UFW, and Fail2Ban
3. Clone the DockPanel repository to `/opt/dockpanel`
4. Build and start the agent, API, and frontend services
5. Configure Nginx as a reverse proxy on port 8443

On ARM64 servers with less than 2GB RAM, the installer automatically uses pre-built binaries instead of compiling from source.

To use pre-built binaries on any architecture (faster, no Rust toolchain needed):

```bash
INSTALL_FROM_RELEASE=1 curl -sL https://dockpanel.dev/install.sh | sudo bash
```

Or clone and run manually:

```bash
git clone https://github.com/ovexro/dockpanel.git /opt/dockpanel
cd /opt/dockpanel
sudo bash scripts/setup.sh
```

To install a specific release rather than the newest one, set `DOCKPANEL_VERSION` to a
release tag. It selects both the source tree and the binaries, so the two cannot drift
apart:

```bash
DOCKPANEL_VERSION=v2.34.2 curl -sL https://dockpanel.dev/install.sh | sudo bash
```

Before 2.35.0 this variable chose the source tree only and the binaries were always the
newest release, so a pinned install ended up mismatched.

## First Login

1. Open your browser and go to `https://YOUR_SERVER_IP:8443`
2. Your browser warns about the certificate — expected, and safe to continue. Without a
   domain there is no way to obtain a trusted certificate, so the installer generates a
   self-signed one rather than serving the account-creation form over plain HTTP
3. You will see the account creation screen
4. Enter your email and password to create the admin account
5. You are signed in to the DockPanel dashboard

If you installed with `PANEL_DOMAIN=your-domain.com`, the panel is at `https://your-domain.com`
on the standard HTTPS port with a trusted Let's Encrypt certificate, and none of the above applies.

## First Steps

After your first login, here is what to do next:

- [ ] **Create your first site** -- Go to Sites, click New Site, enter a domain, and choose a runtime (static, PHP, Node.js, or Python). DockPanel configures Nginx and provisions SSL automatically.
- [ ] **Deploy a Docker app** -- Go to Docker Apps, browse 149 one-click templates across 14 categories (AI, CMS, databases, media, monitoring, and more), and deploy one with a single click.
- [ ] **Enable 2FA** -- Go to Settings and enable TOTP two-factor authentication. Save the 10 recovery codes somewhere safe.
- [ ] **Set up backups** -- Go to Backups and create a backup schedule. Optionally configure an S3-compatible remote destination.
- [ ] **Run diagnostics** -- Check the Dashboard for your server health score, or run `dockpanel diagnose` from the terminal to identify any issues.

## DNS Setup

To serve a site from your DockPanel server, point your domain's DNS to the server.

1. Log in to your domain registrar or DNS provider (Cloudflare, Namecheap, Route53, etc.)
2. Create an **A record** pointing your domain to your server's public IP address:

```
Type: A
Name: example.com (or @ for the root domain)
Value: 203.0.113.10  (your server's IP)
TTL: Auto (or 300)
```

3. If you also want `www.example.com`, add another A record:

```
Type: A
Name: www
Value: 203.0.113.10
TTL: Auto
```

4. Wait for DNS propagation (usually 1-5 minutes, up to 48 hours in rare cases)
5. Create the site in DockPanel with the matching domain -- SSL will be provisioned automatically via Let's Encrypt

DockPanel also has built-in DNS management for Cloudflare and PowerDNS if you want to manage DNS records directly from the panel.
