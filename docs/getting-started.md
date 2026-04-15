# Getting Started

## What is DockPanel?

DockPanel is a free, self-hosted, Docker-native server management panel built in Rust. It lets you manage sites, databases, Docker apps, SSL certificates, backups, email, DNS, and security from a single web interface or CLI. It installs in under 60 seconds, the panel services themselves idle at about ~19MB of RAM (about ~85MB total with the bundled PostgreSQL), and it runs on x86_64 and ARM64 servers with no subscriptions or artificial limits.

## System Requirements

| Requirement | Minimum | Recommended |
|-------------|---------|-------------|
| **OS** | Ubuntu 20.04+, Debian 11+, CentOS 9+, Rocky Linux 9+, Fedora 39+, Amazon Linux 2023 | Ubuntu 22.04 LTS |
| **Architecture** | x86_64 or ARM64 (aarch64) | x86_64 |
| **RAM** | 512 MB | 1 GB+ |
| **Disk** | 10 GB | 20 GB+ |
| **CPU** | 1 core | 2 cores |

Docker and Nginx are installed automatically if not already present.

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

## First Login

1. Open your browser and go to `http://YOUR_SERVER_IP:8443`
2. You will see the account creation screen
3. Enter your email and password to create the admin account
4. You are now logged in to the DockPanel dashboard

If you have a domain pointed to your server, you can access the panel at `https://your-domain.com:8443` after SSL is configured.

## First Steps

After your first login, here is what to do next:

- [ ] **Create your first site** -- Go to Sites, click New Site, enter a domain, and choose a runtime (static, PHP, Node.js, or Python). DockPanel configures Nginx and provisions SSL automatically.
- [ ] **Deploy a Docker app** -- Go to Docker Apps, browse 151 one-click templates across 14 categories (AI, CMS, databases, media, monitoring, and more), and deploy one with a single click.
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
