# WordPress Site Guide

## Prerequisites

- DockPanel installed and running
- A domain with an A record pointing to your server's IP (see [Getting Started](../getting-started.md#dns-setup))
- Port 80 and 443 open in your firewall (DockPanel opens these during install)

## Create a WordPress Site

### From the Panel

1. Go to **Sites** in the sidebar
2. Click **New Site**
3. Fill in the form:
   - **Domain**: `example.com`
   - **Runtime**: `PHP`
   - **CMS**: `WordPress`
4. Click **Create**

### From the CLI

```bash
dockpanel sites create example.com --runtime php --ssl --ssl-email you@example.com
```

The CLI creates the site with PHP and SSL. WordPress CMS installation is available through the panel interface.

## What Happens Automatically

When you create a WordPress site, DockPanel performs these steps in sequence:

1. **Creates the document root** at `/var/www/example.com/public/`
2. **Creates a MySQL database** in a Docker container with auto-generated credentials
3. **Downloads WordPress** to the document root
4. **Configures `wp-config.php`** with the database credentials, table prefix, and security salts
5. **Writes the Nginx config** with PHP-FPM upstream, WordPress-specific rewrite rules, and security headers
6. **Provisions a free SSL certificate** via Let's Encrypt (requires DNS to be pointed)
7. **Reloads Nginx** to apply the configuration

You can watch each step complete in real-time from the panel.

## Post-Install

Once the site is created:

1. Open `https://example.com/wp-admin/install.php` in your browser
2. Complete the WordPress installation wizard (site title, admin username, password, email)
3. Log in at `https://example.com/wp-admin`

### WordPress Toolkit

DockPanel includes a WordPress Toolkit (sidebar > WordPress) that provides:

- **Multi-site dashboard** -- See all WordPress installations on the server
- **Vulnerability scanning** -- Checks plugins against 14 known exploited vulnerabilities
- **Security hardening** -- 7 checks (6 auto-fixable) including file permissions, debug mode, editor access
- **Bulk updates** -- Update plugins, themes, and WordPress core across multiple sites at once

## Troubleshooting

### PHP-FPM not installed

**Symptom**: Site creation fails with a message about PHP-FPM socket not found.

**Fix**: Install PHP from the panel or CLI:

```bash
# From the CLI
dockpanel php install 8.3

# Or from the panel: Settings > Service Installers > PHP-FPM
```

DockPanel validates that PHP-FPM is available before writing the Nginx config and will tell you exactly which version to install.

### SSL provisioning fails

**Symptom**: Site is created but shows HTTP only, or SSL provisioning returns an error.

**Causes and fixes**:

- **DNS not pointed**: The A record for your domain must resolve to this server's IP. Check with `dig example.com +short`.
- **Port 80 blocked**: Let's Encrypt uses HTTP-01 challenges on port 80. Check your firewall: `ufw status`. Port 80 must be open.
- **Rate limit**: Let's Encrypt has a rate limit of 5 duplicate certificates per week. Wait and retry, or use a different subdomain for testing.

Retry SSL provisioning:

```bash
dockpanel ssl provision example.com --email you@example.com --runtime php
```

### 502 Bad Gateway

**Symptom**: The site loads but shows a 502 error.

**Causes and fixes**:

- **PHP-FPM not running**: Check the service status:
  ```bash
  systemctl status php8.3-fpm
  ```
  Restart it if needed:
  ```bash
  systemctl restart php8.3-fpm
  ```

- **Wrong PHP-FPM socket path**: Verify the Nginx config points to the correct socket:
  ```bash
  grep fastcgi_pass /etc/nginx/sites-available/example.com
  ```
  It should match the running PHP-FPM version (e.g., `/run/php/php8.3-fpm.sock`).

- **WordPress memory limit**: Add to `wp-config.php`:
  ```php
  define('WP_MEMORY_LIMIT', '256M');
  ```

### Database connection error

**Symptom**: WordPress shows "Error establishing a database connection".

**Fix**: Check that the MySQL container is running:

```bash
docker ps | grep mysql
```

If it is not running, start it from the panel (Databases page) or restart it:

```bash
docker start <container_id>
```

Verify the credentials in `/var/www/example.com/public/wp-config.php` match the database container's environment.
