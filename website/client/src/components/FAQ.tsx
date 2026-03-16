import { useState } from 'react';

const faqs = [
  {
    q: 'What are the system requirements?',
    a: 'DockPanel runs on any VPS with 512MB RAM, 1 CPU core, and 10GB disk. It supports Ubuntu 20+, Debian 11+, CentOS 9+, Rocky Linux 9+, Amazon Linux 2023, and ARM64 (Raspberry Pi, Oracle Cloud free tier). Docker is installed automatically if not present.',
  },
  {
    q: 'Is DockPanel really free?',
    a: 'Yes. DockPanel is 100% free and open source. There are no premium tiers, no artificial limits, no subscriptions. Every feature works on every installation. Optional paid support plans are available if you need professional help.',
  },
  {
    q: 'How does Docker-native work? Are sites in containers?',
    a: 'Sites run on the host with Nginx, which keeps things fast and simple. Docker is used for databases (MySQL/PostgreSQL containers), 53 one-click app templates (Ghost, Grafana, Nextcloud, n8n, Roundcube, Rspamd, and more), and Docker Compose imports. Containers get health monitoring, CPU/memory limits, and auto reverse proxy with SSL.',
  },
  {
    q: 'What happens if DockPanel goes down?',
    a: 'Your sites continue running. DockPanel manages Nginx configs and Docker containers, but they operate independently. Even if the panel process stops, all your sites, databases, and SSL certificates keep working. The panel auto-restarts via systemd.',
  },
  {
    q: 'Can I run DockPanel on a Raspberry Pi?',
    a: 'Yes. DockPanel compiles to a ~10MB ARM64 binary and uses about 12MB RAM. It runs great on Raspberry Pi 4/5, Oracle Cloud free-tier ARM instances, and any ARM64 server. Same features, same performance.',
  },
  {
    q: 'How is this different from CloudPanel, RunCloud, or cPanel?',
    a: 'DockPanel is fully self-hosted (no SaaS dependency), Docker-native (databases and apps run in containers), and built in Rust (~10MB binary, 12MB RAM vs 250-800MB for PHP panels). It includes features other panels charge for: one-click install for 6 PHP frameworks, 2FA, developer CLI, infrastructure as code, smart diagnostics, auto-healing, 53 app templates, real-time provisioning logs, and more. All free.',
  },
  {
    q: 'Can I manage multiple servers?',
    a: 'Yes, with no limits. Each server runs a lightweight agent that communicates with the central API. You can view per-server metrics, run commands, and manage sites across all your servers from a single dashboard.',
  },
  {
    q: 'Is there a demo I can try?',
    a: 'Yes. Visit demo.dockpanel.dev to explore the full panel interface with sample data — no signup required. You can also install DockPanel on your own server with one command.',
  },
  {
    q: 'Why Rust instead of PHP/Node.js?',
    a: "Rust compiles to a single ~10MB binary with no runtime dependencies. It uses ~12MB of RAM vs ~300-800MB for PHP-based panels. On a $5 VPS with 1GB RAM, that's the difference between running 2 sites and running 20. Rust also eliminates entire classes of security vulnerabilities (buffer overflows, use-after-free) at compile time.",
  },
  {
    q: 'Can DockPanel manage email?',
    a: 'Yes. DockPanel has full email management — one-click install of Postfix + Dovecot + OpenDKIM, mail domains, mailboxes with quotas, aliases, forwarding, autoresponders, DKIM signing, DNS helper (generates MX/SPF/DKIM/DMARC records), and a mail queue viewer. Roundcube webmail and Rspamd spam filter are available as one-click Docker apps.',
  },
  {
    q: 'What gets installed automatically?',
    a: 'The install script sets up everything: Docker, Nginx, PHP-FPM, Certbot (SSL), UFW (firewall), Fail2Ban (intrusion prevention), and the DockPanel agent + API + frontend. Mail server, webmail, and DNS server are optional one-click installs from the panel. Zero manual configuration needed.',
  },
  {
    q: 'Can I install WordPress / Laravel / Drupal with one click?',
    a: 'Yes. DockPanel supports one-click install for 6 frameworks: WordPress, Laravel, Drupal, Joomla, Symfony, and CodeIgniter. Select one, enter your domain, and DockPanel auto-creates the database, downloads the framework, configures everything, and provisions SSL — with real-time progress indicators showing each step as it completes.',
  },
];

export default function FAQ() {
  const [openIndex, setOpenIndex] = useState<number | null>(null);

  return (
    <section id="faq" className="section-alt py-24">
      <div className="mx-auto max-w-3xl px-6">
        <div className="text-center">
          <p className="text-sm font-medium uppercase tracking-wider text-brand-400">FAQ</p>
          <h2 className="mt-3 text-3xl font-bold tracking-tight text-white md:text-4xl">
            Frequently asked questions
          </h2>
        </div>

        <div className="mt-12 space-y-3">
          {faqs.map(({ q, a }, i) => (
            <div
              key={i}
              className="overflow-hidden rounded-xl border border-white/[0.06] bg-white/[0.015] transition-colors hover:border-white/[0.1]"
            >
              <button
                onClick={() => setOpenIndex(openIndex === i ? null : i)}
                className="flex w-full items-center justify-between px-6 py-4 text-left transition hover:bg-white/[0.02]"
              >
                <span className="pr-4 text-sm font-medium text-white">{q}</span>
                <svg
                  className={`h-5 w-5 shrink-0 text-surface-200/40 transition-transform duration-200 ${
                    openIndex === i ? 'rotate-180' : ''
                  }`}
                  fill="none"
                  stroke="currentColor"
                  viewBox="0 0 24 24"
                >
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
                </svg>
              </button>
              <div
                className={`grid transition-all duration-200 ${
                  openIndex === i ? 'grid-rows-[1fr]' : 'grid-rows-[0fr]'
                }`}
              >
                <div className="overflow-hidden">
                  <p className="px-6 pt-1 pb-5 text-sm leading-relaxed text-surface-200/50">{a}</p>
                </div>
              </div>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
