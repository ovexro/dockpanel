const features = [
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M21 12a9 9 0 01-9 9m9-9a9 9 0 00-9-9m9 9H3m9 9a9 9 0 01-9-9m9 9c1.657 0 3-4.03 3-9s-1.343-9-3-9m0 18c-1.657 0-3-4.03-3-9s1.343-9 3-9m-9 9a9 9 0 019-9" />
      </svg>
    ),
    title: 'Site Management',
    description: 'Create and manage websites with automatic Nginx configuration. Support for PHP (multiple versions), Node.js, static sites, and reverse proxies.',
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
    iconBg: 'bg-emerald-500/10',
    iconColor: 'text-emerald-400',
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
    badge: 'Coming soon',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M20 7l-8-4-8 4m16 0l-8 4m8-4v10l-8 4m0-10L4 7m8 4v10M4 7v10l8 4" />
      </svg>
    ),
    title: 'Docker Apps & Compose',
    description: 'One-click app templates (WordPress, Ghost, Redis, Portainer, n8n, and more). Import and deploy docker-compose.yml files with auto reverse proxy and SSL.',
    iconBg: 'bg-emerald-500/10',
    iconColor: 'text-emerald-400',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M19.5 14.25v-2.625a3.375 3.375 0 00-3.375-3.375h-1.5A1.125 1.125 0 0113.5 7.125v-1.5a3.375 3.375 0 00-3.375-3.375H8.25m0 12.75h7.5m-7.5 3H12M10.5 2.25H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 00-9-9z" />
      </svg>
    ),
    title: 'Infrastructure as Code',
    description: 'Export your entire server config as YAML. Version control it, review diffs, apply to new servers. Reproducible infrastructure with dry-run support.',
    iconBg: 'bg-amber-500/10',
    iconColor: 'text-amber-400',
    badge: 'Coming soon',
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
    badge: 'Coming soon',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M5 8h14M5 8a2 2 0 110-4h14a2 2 0 110 4M5 8v10a2 2 0 002 2h10a2 2 0 002-2V8m-9 4h4" />
      </svg>
    ),
    title: 'Backups & S3 Storage',
    description: 'Scheduled backups with S3-compatible remote destinations. Per-site backup policies, one-click restore, and connection testing.',
    iconBg: 'bg-amber-500/10',
    iconColor: 'text-amber-400',
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
    description: 'Runs on Raspberry Pi, Oracle Cloud free-tier ARM instances, and any ARM64 server. Same ~10MB binary, same features, same performance.',
    iconBg: 'bg-emerald-500/10',
    iconColor: 'text-emerald-400',
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
          {features.map(({ icon, title, description, iconBg, iconColor, badge }) => (
            <div
              key={title}
              className="group relative rounded-2xl border border-white/[0.06] bg-white/[0.015] p-6 transition duration-300 hover:border-white/[0.12] hover:bg-white/[0.03] hover:shadow-lg hover:shadow-black/20"
            >
              {badge && (
                <span className="absolute top-4 right-4 rounded-full bg-brand-500/15 px-2 py-0.5 text-[10px] font-medium text-brand-400">
                  {badge}
                </span>
              )}
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
