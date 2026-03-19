const features = [
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M21 12a9 9 0 01-9 9m9-9a9 9 0 00-9-9m9 9H3m9 9a9 9 0 01-9-9m9 9c1.657 0 3-4.03 3-9s-1.343-9-3-9m0 18c-1.657 0-3-4.03-3-9s1.343-9 3-9m-9 9a9 9 0 019-9" />
      </svg>
    ),
    title: 'Site Management',
    description: 'One-click install for WordPress, Laravel, Drupal, Joomla, Symfony, and CodeIgniter. Auto-creates database, configures Nginx, installs the framework, and provisions SSL — with real-time progress indicators.',
    iconBg: 'bg-brand-500/10',
    iconColor: 'text-brand-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
      </svg>
    ),
    title: 'Free SSL Certificates',
    description: "Automatic Let's Encrypt provisioning and renewal. One-click SSL for every site with zero-downtime certificate rotation.",
    iconBg: 'bg-brand-500/10',
    iconColor: 'text-brand-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4m0 5c0 2.21-3.582 4-8 4s-8-1.79-8-4" />
      </svg>
    ),
    title: 'Database Management',
    description: 'MySQL and PostgreSQL via Docker containers. Create databases per site with credential management, one-click backups and restores.',
    iconBg: 'bg-brand-500/10',
    iconColor: 'text-brand-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M6.75 7.5l3 2.25-3 2.25m4.5 0h3m-9 8.25h13.5A2.25 2.25 0 0020.25 18V6a2.25 2.25 0 00-2.25-2.25H6A2.25 2.25 0 003.75 6v12A2.25 2.25 0 006 20.25z" />
      </svg>
    ),
    title: 'Developer CLI',
    description: 'Full-featured dockpanel command for all operations. Manage sites, databases, apps, SSL, backups, and more from your terminal. Scriptable and composable.',
    iconBg: 'bg-violet-500/10',
    iconColor: 'text-violet-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M20 7l-8-4-8 4m16 0l-8 4m8-4v10l-8 4m0-10L4 7m8 4v10M4 7v10l8 4" />
      </svg>
    ),
    title: 'Docker Apps & Compose',
    description: '50+ one-click app templates across 10 categories — databases, CMS, monitoring, analytics, mail, and more. Real-time deploy progress, auto reverse proxy with SSL, container health checks, and CPU/memory resource limits.',
    iconBg: 'bg-brand-500/10',
    iconColor: 'text-brand-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M19.5 14.25v-2.625a3.375 3.375 0 00-3.375-3.375h-1.5A1.125 1.125 0 0113.5 7.125v-1.5a3.375 3.375 0 00-3.375-3.375H8.25m0 12.75h7.5m-7.5 3H12M10.5 2.25H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 00-9-9z" />
      </svg>
    ),
    title: 'Infrastructure as Code',
    description: 'Export your entire server config as YAML. Version control it, review diffs, apply to new servers. Reproducible infrastructure with dry-run support.',
    iconBg: 'bg-orange-500/10',
    iconColor: 'text-orange-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
      </svg>
    ),
    title: 'Monitoring & Alerts',
    description: 'Real-time CPU, RAM, disk, and network dashboards. Uptime monitoring with configurable alert rules. Email, Slack, and Discord notifications.',
    iconBg: 'bg-violet-500/10',
    iconColor: 'text-violet-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M11.42 15.17l-5.385-1.08a.5.5 0 01-.358-.713l.217-.38a.5.5 0 01.525-.247l2.78.556a.5.5 0 00.576-.329l.966-2.9a.5.5 0 01.584-.33l2.78.556m-1.713 4.867l2.78.556a.5.5 0 00.576-.329l.966-2.9a.5.5 0 01.584-.33l2.78.556a.5.5 0 00.576-.329l.966-2.9" />
      </svg>
    ),
    title: 'Smart Diagnostics',
    description: 'Pattern-based log analysis, misconfiguration detection, and resource bottleneck identification. One-click fixes for common server issues.',
    iconBg: 'bg-rose-500/10',
    iconColor: 'text-rose-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M5 8h14M5 8a2 2 0 110-4h14a2 2 0 110 4M5 8v10a2 2 0 002 2h10a2 2 0 002-2V8m-9 4h4" />
      </svg>
    ),
    title: 'Backups & S3 Storage',
    description: 'Scheduled backups with S3-compatible remote destinations. Per-site backup policies, one-click restore, and connection testing.',
    iconBg: 'bg-orange-500/10',
    iconColor: 'text-orange-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z" />
      </svg>
    ),
    title: 'Web Terminal & File Manager',
    description: 'Full SSH terminal in your browser via WebSocket. Built-in file manager with editor — browse, edit, upload, and download files.',
    iconBg: 'bg-brand-500/10',
    iconColor: 'text-brand-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z" />
      </svg>
    ),
    title: 'Security & Scanning',
    description: 'Firewall management, Fail2ban status, SSH hardening. Automated security scanning with file integrity checks. Security score with actionable recommendations.',
    iconBg: 'bg-rose-500/10',
    iconColor: 'text-rose-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M8.288 15.038a5.25 5.25 0 017.424 0M5.106 11.856c3.807-3.808 9.98-3.808 13.788 0M1.924 8.674c5.565-5.565 14.587-5.565 20.152 0M12.53 18.22l-.53.53-.53-.53a.75.75 0 011.06 0z" />
      </svg>
    ),
    title: 'ARM & Homelab Ready',
    description: 'Runs on Raspberry Pi, Oracle Cloud free-tier ARM instances, and any ARM64 server. Same ~20MB binary, same features, same performance.',
    iconBg: 'bg-brand-500/10',
    iconColor: 'text-brand-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M16.5 10.5V6.75a4.5 4.5 0 10-9 0v3.75m-.75 11.25h10.5a2.25 2.25 0 002.25-2.25v-6.75a2.25 2.25 0 00-2.25-2.25H6.75a2.25 2.25 0 00-2.25 2.25v6.75a2.25 2.25 0 002.25 2.25z" />
      </svg>
    ),
    title: '2FA / TOTP Authentication',
    description: 'Two-factor authentication with QR code setup, TOTP verification, and 10 one-time recovery codes. Protect your panel with industry-standard 2FA.',
    iconBg: 'bg-rose-500/10',
    iconColor: 'text-rose-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M11.42 15.17l-5.385-1.08a.5.5 0 01-.358-.713l.217-.38a.5.5 0 01.525-.247l2.78.556a.5.5 0 00.576-.329l.966-2.9a.5.5 0 01.584-.33l2.78.556m-1.713 4.867l2.78.556M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
      </svg>
    ),
    title: 'Auto-Healing Engine',
    description: 'Automatic remediation of crashed services, disk space recovery, and SSL renewal. Runs silently in the background with full audit trail and opt-in control.',
    iconBg: 'bg-orange-500/10',
    iconColor: 'text-orange-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9 12h3.75M9 15h3.75M9 18h3.75m3 .75H18a2.25 2.25 0 002.25-2.25V6.108c0-1.135-.845-2.098-1.976-2.192a48.424 48.424 0 00-1.123-.08m-5.801 0c-.065.21-.1.433-.1.664 0 .414.336.75.75.75h4.5a.75.75 0 00.75-.75 2.25 2.25 0 00-.1-.664m-5.8 0A2.251 2.251 0 0113.5 2.25H15c1.012 0 1.867.668 2.15 1.586m-5.8 0c-.376.023-.75.05-1.124.08C9.095 4.01 8.25 4.973 8.25 6.108V8.25m0 0H4.875c-.621 0-1.125.504-1.125 1.125v11.25c0 .621.504 1.125 1.125 1.125h9.75c.621 0 1.125-.504 1.125-1.125V9.375c0-.621-.504-1.125-1.125-1.125H8.25z" />
      </svg>
    ),
    title: 'Activity & Audit Log',
    description: 'Full audit trail of every action — site creation, SSL changes, database operations, user logins. Filterable, searchable, with timestamps and actor tracking.',
    iconBg: 'bg-violet-500/10',
    iconColor: 'text-violet-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M21.75 6.75v10.5a2.25 2.25 0 01-2.25 2.25h-15a2.25 2.25 0 01-2.25-2.25V6.75m19.5 0A2.25 2.25 0 0019.5 4.5h-15a2.25 2.25 0 00-2.25 2.25m19.5 0v.243a2.25 2.25 0 01-1.07 1.916l-7.5 4.615a2.25 2.25 0 01-2.36 0L3.32 8.91a2.25 2.25 0 01-1.07-1.916V6.75" />
      </svg>
    ),
    title: 'Email Management',
    description: 'Full mail server with one-click install. Domains, mailboxes, aliases, DKIM signing, DNS helper (MX/SPF/DMARC), mail queue, autoresponders, quotas. Roundcube webmail and Rspamd spam filter as Docker apps.',
    iconBg: 'bg-brand-500/10',
    iconColor: 'text-brand-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M3.75 13.5l10.5-11.25L12 10.5h8.25L9.75 21.75 12 13.5H3.75z" />
      </svg>
    ),
    title: 'Zero-Friction Setup',
    description: 'One install script sets up everything — Docker, Nginx, PHP, Certbot, UFW, Fail2Ban. Every additional service (mail, DNS) is one click away from the panel. SSH keys, auto-updates, and IP whitelisting built in.',
    iconBg: 'bg-orange-500/10',
    iconColor: 'text-orange-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M5.25 14.25h13.5m-13.5 0a3 3 0 01-3-3m3 3a3 3 0 100 6h13.5a3 3 0 100-6m-16.5-3a3 3 0 013-3h13.5a3 3 0 013 3m-19.5 0a4.5 4.5 0 01.9-2.7L5.737 5.1a3.375 3.375 0 012.7-1.35h7.126c1.062 0 2.062.5 2.7 1.35l2.587 3.45a4.5 4.5 0 01.9 2.7" />
      </svg>
    ),
    title: 'Multi-Server Management',
    description: 'Manage unlimited remote servers from a single dashboard. Install a lightweight agent, connect via HTTPS, and control sites, apps, and databases across all your servers.',
    iconBg: 'bg-brand-500/10',
    iconColor: 'text-brand-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M18 18.72a9.094 9.094 0 003.741-.479 3 3 0 00-4.682-2.72m.94 3.198l.001.031c0 .225-.012.447-.037.666A11.944 11.944 0 0112 21c-2.17 0-4.207-.576-5.963-1.584A6.062 6.062 0 016 18.719m12 0a5.971 5.971 0 00-.941-3.197m0 0A5.995 5.995 0 0012 12.75a5.995 5.995 0 00-5.058 2.772m0 0a3 3 0 00-4.681 2.72 8.986 8.986 0 003.74.477m.94-3.197a5.971 5.971 0 00-.94 3.197M15 6.75a3 3 0 11-6 0 3 3 0 016 0zm6 3a2.25 2.25 0 11-4.5 0 2.25 2.25 0 014.5 0zm-13.5 0a2.25 2.25 0 11-4.5 0 2.25 2.25 0 014.5 0z" />
      </svg>
    ),
    title: 'Reseller & White-Label',
    description: 'Admin → Reseller → User hierarchy with quotas and server allocation. Per-reseller branding: custom logo, panel name, accent color, and option to hide DockPanel branding entirely.',
    iconBg: 'bg-violet-500/10',
    iconColor: 'text-violet-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M15.75 5.25a3 3 0 013 3m3 0a6 6 0 01-7.029 5.912c-.563-.097-1.159.026-1.563.43L10.5 17.25H8.25v2.25H6v2.25H2.25v-2.818c0-.597.237-1.17.659-1.591l6.499-6.499c.404-.404.527-1 .43-1.563A6 6 0 1121.75 8.25z" />
      </svg>
    ),
    title: 'OAuth / SSO Login',
    description: 'Login via Google, GitHub, or GitLab with OAuth 2.0. Auto-create users on first login, CSRF protection, and secure token handling. No more password fatigue.',
    iconBg: 'bg-rose-500/10',
    iconColor: 'text-rose-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9.75 3.104v5.714a2.25 2.25 0 01-.659 1.591L5 14.5M9.75 3.104c-.251.023-.501.05-.75.082m.75-.082a24.301 24.301 0 014.5 0m0 0v5.714c0 .597.237 1.17.659 1.591L19.8 15.3M14.25 3.104c.251.023.501.05.75.082M19.8 15.3l-1.57.393A9.065 9.065 0 0112 15a9.065 9.065 0 00-6.23.693L5 14.5m14.8.8l1.402 1.402c1.232 1.232.65 3.318-1.067 3.611A48.309 48.309 0 0112 21c-2.773 0-5.491-.235-8.135-.687-1.718-.293-2.3-2.379-1.067-3.61L5 14.5" />
      </svg>
    ),
    title: 'Nixpacks Auto-Build',
    description: 'Deploy any app without a Dockerfile. Nixpacks auto-detects 30+ languages (Node, Python, Go, Rust, Ruby, Java, and more) and builds optimized Docker images automatically.',
    iconBg: 'bg-orange-500/10',
    iconColor: 'text-orange-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M7.5 21L3 16.5m0 0L7.5 12M3 16.5h13.5m0-13.5L21 7.5m0 0L16.5 12M21 7.5H7.5" />
      </svg>
    ),
    title: 'Migration Wizard',
    description: 'Import sites, databases, and email from cPanel, Plesk, or HestiaCP. Upload your backup, review what was found, and import selected items with real-time progress.',
    iconBg: 'bg-brand-500/10',
    iconColor: 'text-brand-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M12 21a9.004 9.004 0 008.716-6.747M12 21a9.004 9.004 0 01-8.716-6.747M12 21c2.485 0 4.5-4.03 4.5-9S14.485 3 12 3m0 18c-2.485 0-4.5-4.03-4.5-9S9.515 3 12 3m0 0a8.997 8.997 0 017.843 4.582M12 3a8.997 8.997 0 00-7.843 4.582m15.686 0A11.953 11.953 0 0112 10.5c-2.998 0-5.74-1.1-7.843-2.918m15.686 0A8.959 8.959 0 0121 12c0 .778-.099 1.533-.284 2.253m0 0A17.919 17.919 0 0112 16.5c-3.162 0-6.133-.815-8.716-2.247m0 0A9.015 9.015 0 003 12c0-1.605.42-3.113 1.157-4.418" />
      </svg>
    ),
    title: 'WordPress Toolkit',
    description: 'Multi-site WP dashboard with vulnerability scanning against known exploits. Security hardening with one-click fixes. Bulk update plugins, themes, and core across all your WordPress sites.',
    iconBg: 'bg-violet-500/10',
    iconColor: 'text-violet-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M14.25 9.75L16.5 12l-2.25 2.25m-4.5 0L7.5 12l2.25-2.25M6 20.25h12A2.25 2.25 0 0020.25 18V6A2.25 2.25 0 0018 3.75H6A2.25 2.25 0 003.75 6v12A2.25 2.25 0 006 20.25z" />
      </svg>
    ),
    title: 'Extension / Plugin API',
    description: 'Webhook-based integrations with HMAC-signed event delivery. Register extensions that receive site, deploy, backup, and security events. Scoped API keys for programmatic access.',
    iconBg: 'bg-rose-500/10',
    iconColor: 'text-rose-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M7.5 21L3 16.5m0 0L7.5 12M3 16.5h13.5m0-13.5L21 7.5m0 0L16.5 12M21 7.5H7.5" />
      </svg>
    ),
    title: 'Traefik Reverse Proxy',
    description: 'Choose nginx or Traefik for Docker app routing. Traefik provides native Docker service discovery, automatic Let\'s Encrypt SSL, and a built-in routing dashboard.',
    iconBg: 'bg-orange-500/10',
    iconColor: 'text-orange-400',
  },
];

export default function Features() {
  return (
    <section id="features" className="section-alt py-24">
      <div className="mx-auto max-w-6xl px-6">
        <div className="text-center">
          <p className="text-sm font-medium uppercase tracking-wider text-brand-400">Features</p>
          <h2 className="mt-3 text-3xl font-bold tracking-tight text-white md:text-4xl">
            Everything you need — nothing you don't
          </h2>
          <p className="mx-auto mt-4 max-w-2xl text-surface-200/60">
            No bloat, no legacy PHP, no 500MB installations, no subscriptions.
            Just the tools you actually use, built with modern technology. All features included for free.
          </p>
        </div>

        <div className="mt-16 grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
          {features.map(({ icon, title, description, iconBg, iconColor }) => (
            <div
              key={title}
              className="group relative rounded-2xl border border-white/[0.06] bg-white/[0.015] p-6 transition duration-300 hover:border-white/[0.12] hover:bg-white/[0.03] hover:shadow-lg hover:shadow-black/20"
            >
              <div className={`flex h-11 w-11 items-center justify-center rounded-xl ${iconBg} ${iconColor} transition duration-300`}>
                {icon}
              </div>
              <h3 className="mt-4 text-base font-semibold text-white">{title}</h3>
              <p className="mt-2 text-sm leading-relaxed text-surface-200/50">{description}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
