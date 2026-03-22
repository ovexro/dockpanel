import { useState } from 'react';
import { motion, AnimatePresence } from 'motion/react';
import { Link } from 'react-router-dom';
import {
  Terminal, Server, ShieldCheck, Database, Globe, Lock, Box,
  FileCode2, Activity, Stethoscope, HardDriveDownload, Cpu,
  KeyRound, Wrench, ClipboardList, Mail, Zap, Users, LogIn,
  Package, ArrowRightLeft, LayoutTemplate, Puzzle, Network,
  ChevronDown, Check, Github, Play, Copy, CheckCircle2, X,
  Palette
} from 'lucide-react';

const features = [
  { icon: Globe, title: 'Site Management', desc: 'One-click install for WordPress, Laravel, Drupal, Joomla, Symfony, and CodeIgniter. Auto-creates database, configures Nginx, installs the framework, and provisions SSL.' },
  { icon: Lock, title: 'Free SSL Certificates', desc: "Automatic Let's Encrypt provisioning and renewal. One-click SSL for every site with zero-downtime certificate rotation." },
  { icon: Database, title: 'Database Management', desc: 'MySQL and PostgreSQL via Docker containers. Built-in SQL browser for both engines. Per-site credentials, one-click backups and restores.' },
  { icon: Terminal, title: 'Developer CLI', desc: 'Full-featured dockpanel command for all operations. Manage sites, databases, apps, SSL, backups, and more from your terminal. Scriptable and composable.' },
  { icon: Box, title: 'Docker Apps & Compose', desc: '54 one-click app templates across 10 categories — databases, CMS, monitoring, analytics, mail, and more. Real-time deploy progress, auto reverse proxy with SSL.' },
  { icon: FileCode2, title: 'Infrastructure as Code', desc: 'Export your entire server config as YAML. Version control it, review diffs, apply to new servers. Reproducible infrastructure with dry-run support.' },
  { icon: Activity, title: 'Monitoring & Incidents', desc: 'Uptime monitoring with incident management lifecycle (investigating → resolved → postmortem). Public status page with component groups, subscriber notifications, and incident timeline. Slack, Discord, PagerDuty alerts.' },
  { icon: Stethoscope, title: 'Smart Diagnostics', desc: 'Pattern-based log analysis, misconfiguration detection, and resource bottleneck identification. One-click fixes for common server issues.' },
  { icon: HardDriveDownload, title: 'Backup Orchestrator', desc: 'Database, volume & site backups with AES-256 encryption, automatic restore verification, and cross-resource policies. S3, B2, GCS, and SFTP destinations. Health dashboard with staleness alerts.' },
  { icon: Terminal, title: 'Web Terminal & File Manager', desc: 'Full SSH terminal in your browser with tabs, themes, sharing, and session recording. Built-in file manager — browse, edit, upload, and download files.' },
  { icon: ShieldCheck, title: 'Secrets Manager', desc: 'AES-256-GCM encrypted vaults for API keys, passwords, certificates. Version history, auto-inject into .env on deploy, masked API responses, CLI pull. No more plaintext secrets.' },
  { icon: Cpu, title: 'ARM & Homelab Ready', desc: 'Runs on Raspberry Pi, Oracle Cloud free-tier ARM instances, and any ARM64 server. Same ~35MB binaries, same features, same performance.' },
  { icon: KeyRound, title: '2FA / TOTP Authentication', desc: 'Two-factor authentication with QR code setup, TOTP verification, and 10 one-time recovery codes. Protect your panel with industry-standard 2FA.' },
  { icon: Wrench, title: 'Auto-Healing Engine', desc: 'Automatic remediation of crashed services, disk space recovery, and SSL renewal. Runs silently in the background with full audit trail and opt-in control.' },
  { icon: ClipboardList, title: 'Activity & Audit Log', desc: 'Full audit trail of every action — site creation, SSL changes, database operations, user logins. Filterable, searchable, with timestamps and actor tracking.' },
  { icon: Mail, title: 'Email Management', desc: "Full mail server with one-click install. Domains, mailboxes, aliases, DKIM signing, DNS helper, mail queue, autoresponders, quotas. Roundcube webmail and Rspamd spam filter." },
  { icon: Zap, title: 'Zero-Friction Setup', desc: 'One install script sets up everything — Docker, Nginx, PHP, Certbot, UFW, Fail2Ban. Every additional service is one click away. SSH keys, auto-updates built in.' },
  { icon: Server, title: 'Multi-Server Management', desc: 'Manage unlimited remote servers from a single dashboard. Install a lightweight agent, connect via HTTPS, and control sites, apps, and databases across all your servers.' },
  { icon: Users, title: 'Reseller & White-Label', desc: 'Admin → Reseller → User hierarchy with quotas and server allocation. Per-reseller branding: custom logo, panel name, accent color, and option to hide branding.' },
  { icon: LogIn, title: 'OAuth / SSO Login', desc: 'Login via Google, GitHub, or GitLab with OAuth 2.0. Auto-create users on first login, CSRF protection, and secure token handling. No more password fatigue.' },
  { icon: Package, title: 'Nixpacks Auto-Build', desc: 'Deploy any app without a Dockerfile. Nixpacks auto-detects 30+ languages and builds optimized Docker images. Blue-green zero-downtime deployments included.' },
  { icon: ArrowRightLeft, title: 'Migration Wizard', desc: 'Import sites, databases, and email from cPanel, Plesk, or HestiaCP. Upload your backup, review what was found, and import selected items with real-time progress.' },
  { icon: LayoutTemplate, title: 'WordPress Toolkit', desc: 'Multi-site WP dashboard with vulnerability scanning against known exploits. Security hardening with one-click fixes. Bulk update plugins, themes, and core.' },
  { icon: Puzzle, title: 'Webhook Gateway', desc: 'Receive external webhooks, inspect requests, route to destinations with JSON filtering. HMAC-SHA256/SHA1 verification, retry with backoff, replay past deliveries. Plus outbound extension webhooks with scoped API keys.' },
  { icon: Network, title: 'Traefik Reverse Proxy', desc: "Choose nginx or Traefik for Docker app routing. Traefik provides native Docker service discovery, automatic Let's Encrypt SSL, and a built-in routing dashboard." },
  { icon: Palette, title: '6 Themes + 3 Layouts', desc: 'Terminal, Midnight, Ember, Arctic, Clean, and Clean Dark themes — each with Sidebar, Compact, and Topbar layouts. No other panel lets you choose how your dashboard looks and feels.' },
];

const faqs = [
  { q: 'What are the system requirements?', a: 'DockPanel runs on any VPS with 512MB RAM, 1 CPU core, and 10GB disk. It supports Ubuntu 20+, Debian 11+, CentOS 9+, Rocky Linux 9+, Amazon Linux 2023, and ARM64 (Raspberry Pi, Oracle Cloud free tier). Docker is installed automatically if not present.' },
  { q: 'Is DockPanel really free?', a: 'Yes. DockPanel is 100% free and open source under the MIT License. There are no artificial limits, no premium features hidden behind a paywall, and no subscriptions required. Every feature works on every installation.' },
  { q: 'How does Docker-native work? Are sites in containers?', a: 'Sites run on the host with Nginx, which keeps things fast and simple. Docker is used for databases (MySQL/PostgreSQL containers), 54 one-click app templates (Ghost, Grafana, Nextcloud, n8n, Roundcube, Rspamd, and more), and Docker Compose imports. Containers get health monitoring, CPU/memory limits, and auto reverse proxy with SSL.' },
  { q: 'What happens if DockPanel goes down?', a: 'Your sites continue running. DockPanel manages Nginx configs and Docker containers, but they operate independently. Even if the panel process stops, all your sites, databases, and SSL certificates keep working. The panel auto-restarts via systemd.' },
  { q: 'Can I run DockPanel on a Raspberry Pi?', a: 'Yes. DockPanel compiles to ~35MB total binaries (agent, API, CLI) and uses about 57MB RAM. It runs great on Raspberry Pi 4/5, Oracle Cloud free-tier ARM instances, and any ARM64 server. Same features, same performance.' },
  { q: 'How is this different from CloudPanel, RunCloud, or cPanel?', a: 'Dramatically. cPanel uses 800MB of RAM, costs $15/month, and has no Docker support. CloudPanel is free but has no Git deploy, no Docker apps, no CLI, no multi-server, no reseller accounts. RunCloud charges $8/month for features DockPanel includes for free. DockPanel runs on 57MB of RAM, includes 54 Docker app templates, blue-green Git deploy, a full CLI, Infrastructure as Code, multi-server management, reseller accounts with white-label, and a WordPress toolkit. No other free panel has anything close to this feature set.' },
  { q: 'Can I manage multiple servers?', a: 'Yes, with no limits. Each server runs a lightweight agent that communicates with the central API. You can view per-server metrics, run commands, and manage sites across all your servers from a single dashboard.' },
  { q: 'Is there a demo I can try?', a: 'Yes. Visit demo.dockpanel.dev to explore the full panel interface with sample data — no signup required. You can also install DockPanel on your own server with one command.' },
  { q: 'Why Rust instead of PHP/Node.js?', a: "Rust compiles to compact binaries (~35MB total for agent, API, and CLI) with no runtime dependencies. It uses ~57MB of RAM vs ~300-800MB for PHP-based panels. On a $5 VPS with 1GB RAM, that's the difference between running 2 sites and running 20. Rust also eliminates entire classes of security vulnerabilities (buffer overflows, use-after-free) at compile time." },
  { q: 'Can DockPanel manage email?', a: "Yes. DockPanel has full email management — one-click install of Postfix + Dovecot + OpenDKIM, mail domains, mailboxes with quotas, aliases, forwarding, autoresponders, DKIM signing, DNS helper (generates MX/SPF/DKIM/DMARC records), and a mail queue viewer. Roundcube webmail and Rspamd spam filter are available as one-click Docker apps." },
  { q: 'What gets installed automatically?', a: 'The install script sets up everything: Docker, Nginx, PHP-FPM, Certbot (SSL), UFW (firewall), Fail2Ban (intrusion prevention), and the DockPanel agent + API + frontend. Mail server, webmail, and DNS server are optional one-click installs from the panel. Zero manual configuration needed.' },
  { q: 'Can I install WordPress / Laravel / Drupal with one click?', a: 'Yes. DockPanel supports one-click install for 6 frameworks: WordPress, Laravel, Drupal, Joomla, Symfony, and CodeIgniter. Select one, enter your domain, and DockPanel auto-creates the database, downloads the framework, configures everything, and provisions SSL — with real-time progress indicators showing each step as it completes.' },
];

export default function Landing() {
  const [copied, setCopied] = useState(false);
  const [openFaq, setOpenFaq] = useState<number | null>(8);

  const handleCopy = () => {
    navigator.clipboard.writeText('curl -sL https://dockpanel.dev/install.sh | sudo bash');
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="min-h-screen bg-zinc-950 text-zinc-300 font-sans selection:bg-emerald-500/30">
      {/* Navbar */}
      <nav className="sticky top-0 z-50 border-b border-zinc-800/50 bg-zinc-950/80 backdrop-blur-md">
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 h-16 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2">
            <div className="w-8 h-8 rounded-lg bg-emerald-500 flex items-center justify-center">
              <Server className="w-5 h-5 text-zinc-950" />
            </div>
            <span className="text-xl font-bold text-white tracking-tight">DockPanel</span>
          </Link>
          <div className="hidden md:flex items-center gap-8 text-sm font-medium">
            <a href="#features" className="hover:text-white transition-colors">Features</a>
            <a href="#how-it-works" className="hover:text-white transition-colors">How it works</a>
            <a href="#pricing" className="hover:text-white transition-colors">Pricing</a>
            <a href="#faq" className="hover:text-white transition-colors">FAQ</a>
            <a href="https://github.com/ovexro/dockpanel#readme" className="hover:text-white transition-colors">Docs</a>
          </div>
          <div className="flex items-center gap-4">
            <a href="https://github.com/ovexro/dockpanel" className="hidden sm:flex items-center gap-2 text-sm font-medium hover:text-white transition-colors">
              <Github className="w-4 h-4" />
              GitHub
            </a>
            <a href="https://demo.dockpanel.dev" className="bg-white text-zinc-950 px-4 py-2 rounded-md text-sm font-semibold hover:bg-zinc-200 transition-colors">
              Demo
            </a>
          </div>
        </div>
      </nav>

      {/* Hero */}
      <section className="relative pt-24 pb-32 overflow-hidden">
        <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_top,_var(--tw-gradient-stops))] from-emerald-900/20 via-zinc-950 to-zinc-950"></div>
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 relative z-10 text-center">
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.5 }}
          >
            <span className="inline-flex items-center gap-2 px-3 py-1 rounded-full bg-emerald-500/10 text-emerald-400 text-sm font-medium mb-8 border border-emerald-500/20">
              <span className="w-2 h-2 rounded-full bg-emerald-500 animate-pulse"></span>
              100% free &amp; open source — no subscriptions, no limits
            </span>
            <h1 className="text-5xl md:text-7xl font-extrabold text-white tracking-tight mb-4 leading-tight">
              The server panel <br className="hidden sm:block" />
              <span className="text-transparent bg-clip-text bg-gradient-to-r from-emerald-400 to-cyan-400">that does everything.</span>
            </h1>
            <p className="text-lg md:text-xl font-semibold text-zinc-400 mb-6 tracking-wide">
              Your server. Your rules. Your panel.
            </p>
            <p className="max-w-2xl mx-auto text-lg md:text-xl text-zinc-400 mb-10 leading-relaxed">
              54 Docker app templates. Git deploy with zero-downtime. Multi-server management. Reseller accounts. A full CLI. All running on 57MB of RAM. No vendor lock-in. All free. No other panel comes close.
            </p>

            <div className="max-w-2xl mx-auto mb-12">
              <div className="flex items-center bg-zinc-900 border border-zinc-800 rounded-lg p-1 shadow-2xl">
                <div className="flex items-center px-4 text-zinc-500 font-mono text-sm select-none">
                  $
                </div>
                <code className="flex-1 text-left text-emerald-400 font-mono text-sm sm:text-base overflow-x-auto whitespace-nowrap py-3">
                  curl -sL https://dockpanel.dev/install.sh | sudo bash
                </code>
                <button
                  onClick={handleCopy}
                  className="flex items-center gap-2 px-4 py-3 bg-zinc-800 hover:bg-zinc-700 text-white rounded-md text-sm font-medium transition-colors"
                >
                  {copied ? <Check className="w-4 h-4 text-emerald-400" /> : <Copy className="w-4 h-4" />}
                  <span className="hidden sm:inline">{copied ? 'Copied' : 'Copy'}</span>
                </button>
              </div>
              <p className="text-sm text-zinc-500 mt-4">
                Works on Ubuntu 20+, Debian 11+, CentOS 9+, Rocky Linux 9+, Amazon Linux 2023 — x86_64 &amp; ARM64
              </p>
            </div>

            <div className="flex flex-col sm:flex-row items-center justify-center gap-4">
              <a href="https://demo.dockpanel.dev" className="w-full sm:w-auto flex items-center justify-center gap-2 bg-emerald-500 hover:bg-emerald-600 text-zinc-950 px-6 py-3 rounded-lg font-semibold transition-colors">
                <Play className="w-4 h-4 fill-current" />
                Live Demo
              </a>
              <a href="https://github.com/ovexro/dockpanel" className="w-full sm:w-auto flex items-center justify-center gap-2 bg-zinc-800 hover:bg-zinc-700 text-white px-6 py-3 rounded-lg font-semibold transition-colors">
                <Github className="w-4 h-4" />
                View on GitHub
              </a>
            </div>
          </motion.div>

          {/* Stats */}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.5, delay: 0.2 }}
            className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-7 gap-4 max-w-6xl mx-auto mt-20"
          >
            {[
              { label: 'Total binaries', value: '~35MB' },
              { label: 'RAM usage', value: '~57MB' },
              { label: 'Install time', value: '<60s' },
              { label: 'Homelab ready', value: 'ARM64' },
              { label: 'API Endpoints', value: '371' },
              { label: 'E2E Tests', value: '116' },
              { label: 'App Templates', value: '54' },
            ].map((stat, i) => (
              <div key={i} className="bg-zinc-900/50 border border-zinc-800/50 rounded-xl p-6 backdrop-blur-sm">
                <div className="text-3xl font-bold text-white mb-1">{stat.value}</div>
                <div className="text-sm text-zinc-400">{stat.label}</div>
              </div>
            ))}
          </motion.div>
        </div>
      </section>

      {/* Dashboard Preview */}
      <section className="max-w-6xl mx-auto px-4 sm:px-6 lg:px-8 -mt-10 relative z-20 mb-32">
        <motion.div
          initial={{ opacity: 0, y: 40 }}
          whileInView={{ opacity: 1, y: 0 }}
          viewport={{ once: true }}
          transition={{ duration: 0.7 }}
          className="rounded-2xl border border-zinc-800 bg-zinc-950 shadow-2xl overflow-hidden"
        >
          {/* Browser Chrome */}
          <div className="h-12 border-b border-zinc-800 bg-zinc-900 flex items-center px-4 gap-4">
            <div className="flex gap-2">
              <div className="w-3 h-3 rounded-full bg-red-500/80"></div>
              <div className="w-3 h-3 rounded-full bg-yellow-500/80"></div>
              <div className="w-3 h-3 rounded-full bg-green-500/80"></div>
            </div>
            <div className="flex-1 flex justify-center">
              <div className="bg-zinc-950 border border-zinc-800 rounded-md px-4 py-1 text-xs text-zinc-500 font-mono flex items-center gap-2">
                <Lock className="w-3 h-3 text-emerald-500" />
                your-server:8443
              </div>
            </div>
          </div>
          {/* App UI */}
          <div className="flex h-[600px]">
            {/* Sidebar */}
            <div className="w-64 border-r border-zinc-800 bg-zinc-900/30 p-4 hidden md:flex flex-col gap-1">
              <div className="flex items-center gap-2 px-2 py-4 mb-4">
                <div className="w-6 h-6 rounded bg-emerald-500 flex items-center justify-center">
                  <Server className="w-3 h-3 text-zinc-950" />
                </div>
                <span className="font-bold text-white">DockPanel</span>
              </div>
              {[
                { icon: Activity, label: 'Dashboard', active: true },
                { icon: Globe, label: 'Sites' },
                { icon: Database, label: 'Databases' },
                { icon: Box, label: 'Docker Apps' },
                { icon: ShieldCheck, label: 'Security' },
                { icon: Terminal, label: 'Terminal' },
                { icon: Stethoscope, label: 'Diagnostics' },
              ].map((item, i) => (
                <div key={i} className={`flex items-center gap-3 px-3 py-2 rounded-md text-sm font-medium ${item.active ? 'bg-emerald-500/10 text-emerald-400' : 'text-zinc-400'}`}>
                  <item.icon className="w-4 h-4" />
                  {item.label}
                </div>
              ))}
            </div>
            {/* Main Content */}
            <div className="flex-1 bg-zinc-950 p-8 overflow-hidden">
              <h2 className="text-2xl font-bold text-white mb-8">Dashboard</h2>
              <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6">
                {[
                  { label: 'Active Sites', value: '12', sub: 'All healthy', icon: Globe, color: 'text-blue-400', bg: 'bg-blue-400/10' },
                  { label: 'SSL Certs', value: '12', sub: 'Auto-renew', icon: Lock, color: 'text-emerald-400', bg: 'bg-emerald-400/10' },
                  { label: 'Databases', value: '8', sub: '2.1 GB used', icon: Database, color: 'text-purple-400', bg: 'bg-purple-400/10' },
                  { label: 'Docker Apps', value: '5', sub: 'All running', icon: Box, color: 'text-orange-400', bg: 'bg-orange-400/10' },
                  { label: 'Backups', value: '47', sub: 'Last: 2h ago', icon: HardDriveDownload, color: 'text-cyan-400', bg: 'bg-cyan-400/10' },
                  { label: 'CPU / RAM', value: '3%', sub: '57 MB used', icon: Cpu, color: 'text-pink-400', bg: 'bg-pink-400/10' },
                ].map((card, i) => (
                  <div key={i} className="bg-zinc-900 border border-zinc-800 rounded-xl p-6">
                    <div className="flex justify-between items-start mb-4">
                      <div className={`w-10 h-10 rounded-lg ${card.bg} flex items-center justify-center`}>
                        <card.icon className={`w-5 h-5 ${card.color}`} />
                      </div>
                    </div>
                    <div className="text-3xl font-bold text-white mb-1">{card.value}</div>
                    <div className="text-sm font-medium text-zinc-400 mb-1">{card.label}</div>
                    <div className="text-xs text-zinc-500">{card.sub}</div>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </motion.div>
      </section>

      {/* Features */}
      <section id="features" className="py-24 bg-zinc-900/30 border-y border-zinc-800/50">
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
          <div className="text-center max-w-3xl mx-auto mb-20">
            <h2 className="text-3xl md:text-5xl font-bold text-white mb-6 tracking-tight">A massive feature set. Zero monthly fees.</h2>
            <p className="text-lg text-zinc-400">
              25 integrated systems. 371 API endpoints. Features that competitors charge $8-15/month for — Git deploy, multi-server, reseller accounts, Docker orchestration, WordPress toolkit — all included for free. Built in Rust, not legacy PHP.
            </p>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-x-8 gap-y-12">
            {features.map((feature, i) => (
              <div key={i} className="flex gap-4">
                <div className="flex-shrink-0 mt-1">
                  <div className="w-10 h-10 rounded-lg bg-zinc-800 border border-zinc-700 flex items-center justify-center">
                    <feature.icon className="w-5 h-5 text-emerald-400" />
                  </div>
                </div>
                <div>
                  <h3 className="text-lg font-semibold text-white mb-2">{feature.title}</h3>
                  <p className="text-sm text-zinc-400 leading-relaxed">{feature.desc}</p>
                </div>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* How it works */}
      <section id="how-it-works" className="py-32">
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
          <div className="text-center max-w-3xl mx-auto mb-20">
            <h2 className="text-sm font-bold text-emerald-400 uppercase tracking-widest mb-2">How it works</h2>
            <h3 className="text-3xl md:text-5xl font-bold text-white tracking-tight">Up and running in under 60 seconds</h3>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-3 gap-12 relative">
            <div className="hidden md:block absolute top-8 left-[10%] right-[10%] h-px bg-gradient-to-r from-transparent via-zinc-700 to-transparent"></div>

            {[
              {
                step: '01',
                title: 'Install with one command',
                desc: 'Run the install script on any fresh VPS. DockPanel detects your OS, installs Docker if needed, and sets everything up automatically.',
                code: '$ curl -sL https://dockpanel.dev/install.sh | sudo bash'
              },
              {
                step: '02',
                title: 'Create your account',
                desc: "Open your browser and navigate to your server's panel URL. Complete the one-time setup wizard to create your admin account.",
                code: 'https://panel.your-domain.com'
              },
              {
                step: '03',
                title: 'Add your first site',
                desc: 'Click "New Site" in the dashboard, enter your domain name, and DockPanel configures Nginx, provisions a free SSL certificate, and gets your site live.',
                code: 'Dashboard → Sites → New Site'
              }
            ].map((step, i) => (
              <div key={i} className="relative z-10 flex flex-col items-center text-center">
                <div className="w-16 h-16 rounded-full bg-zinc-950 border-2 border-zinc-800 flex items-center justify-center text-xl font-bold text-emerald-400 mb-6 shadow-xl">
                  {step.step}
                </div>
                <h4 className="text-xl font-bold text-white mb-4">{step.title}</h4>
                <p className="text-zinc-400 mb-6 flex-1">{step.desc}</p>
                <div className="w-full bg-zinc-900 border border-zinc-800 rounded-lg p-4 text-sm font-mono text-zinc-300 overflow-hidden text-ellipsis whitespace-nowrap">
                  {step.code}
                </div>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* Comparison */}
      <section className="py-24 bg-zinc-900/30 border-y border-zinc-800/50 overflow-hidden">
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
          <div className="text-center max-w-3xl mx-auto mb-16">
            <h2 className="text-sm font-bold text-emerald-400 uppercase tracking-widest mb-2">Comparison</h2>
            <h3 className="text-3xl md:text-5xl font-bold text-white tracking-tight mb-6">10x lighter. 10x more features.</h3>
            <p className="text-lg text-zinc-400">
              cPanel uses 800MB of RAM and charges $15/month. CloudPanel is free but has no Docker, no Git deploy, no CLI. DockPanel gives you more than any of them — on 57MB of RAM — for free.
            </p>
          </div>

          <div className="overflow-x-auto">
            <table className="w-full text-left border-collapse min-w-[800px]">
              <thead>
                <tr>
                  <th className="p-4 border-b border-zinc-800 text-sm font-semibold text-zinc-400 uppercase tracking-wider">Panel</th>
                  <th className="p-4 border-b border-zinc-800 text-sm font-semibold text-zinc-400 uppercase tracking-wider">RAM Usage</th>
                  <th className="p-4 border-b border-zinc-800 text-sm font-semibold text-zinc-400 uppercase tracking-wider">Install Time</th>
                  <th className="p-4 border-b border-zinc-800 text-sm font-semibold text-zinc-400 uppercase tracking-wider">Disk Size</th>
                  <th className="p-4 border-b border-zinc-800 text-sm font-semibold text-zinc-400 uppercase tracking-wider">Price</th>
                  <th className="p-4 border-b border-zinc-800 text-sm font-semibold text-zinc-400 uppercase tracking-wider">Built With</th>
                  <th className="p-4 border-b border-zinc-800 text-sm font-semibold text-zinc-400 uppercase tracking-wider text-center">Docker-Native</th>
                  <th className="p-4 border-b border-zinc-800 text-sm font-semibold text-zinc-400 uppercase tracking-wider text-center">Self-Hosted</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-zinc-800/50">
                {[
                  { name: 'cPanel', ram: '~800 MB', time: '~45 min', disk: '~2 GB', price: '$15/mo', built: 'Perl/PHP', docker: false, self: true },
                  { name: 'Plesk', ram: '~512 MB', time: '~20 min', disk: '~1.5 GB', price: '$10/mo', built: 'PHP', docker: false, self: true },
                  { name: 'RunCloud', ram: 'N/A (SaaS)', time: '~5 min', disk: 'Agent', price: '$8/mo', built: 'PHP', docker: false, self: false },
                  { name: 'CloudPanel', ram: '~250 MB', time: '~10 min', disk: '~600 MB', price: 'Free', built: 'PHP', docker: false, self: true },
                  { name: 'HestiaCP', ram: '~512 MB', time: '~2 min', disk: '~2 GB', price: 'Free', built: 'PHP', docker: false, self: true },
                  { name: 'DockPanel', ram: '~57 MB', time: '<60 sec', disk: '~35 MB', price: 'Free', built: 'Rust', docker: true, self: true, highlight: true },
                ].map((row, i) => (
                  <tr key={i} className={row.highlight ? 'bg-emerald-500/5' : ''}>
                    <td className={`p-4 font-medium ${row.highlight ? 'text-emerald-400' : 'text-white'}`}>{row.name}</td>
                    <td className="p-4 text-zinc-400">{row.ram}</td>
                    <td className="p-4 text-zinc-400">{row.time}</td>
                    <td className="p-4 text-zinc-400">{row.disk}</td>
                    <td className="p-4 text-zinc-400">{row.price}</td>
                    <td className="p-4 text-zinc-400">{row.built}</td>
                    <td className="p-4 text-center">
                      {row.docker ? <CheckCircle2 className="w-5 h-5 text-emerald-500 mx-auto" /> : <X className="w-5 h-5 text-zinc-600 mx-auto" />}
                    </td>
                    <td className="p-4 text-center">
                      {row.self ? <CheckCircle2 className="w-5 h-5 text-emerald-500 mx-auto" /> : <X className="w-5 h-5 text-zinc-600 mx-auto" />}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      </section>

      {/* Pricing */}
      <section id="pricing" className="py-32">
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
          <div className="text-center max-w-3xl mx-auto mb-16">
            <h2 className="text-sm font-bold text-emerald-400 uppercase tracking-widest mb-2">Pricing</h2>
            <h3 className="text-3xl md:text-5xl font-bold text-white tracking-tight mb-6">Free. Forever. Everything included.</h3>
            <p className="text-lg text-zinc-400">
              No "starter" tier. No "upgrade for multi-server." No "contact sales for reseller." Every single feature — all 25 systems, all 371 endpoints, all 54 templates — is free and open source under MIT. This is the panel that paid alternatives don't want you to find.
            </p>
          </div>

          <div className="max-w-4xl mx-auto">
            <div className="bg-zinc-900 border border-zinc-800 rounded-3xl p-8 md:p-12 shadow-2xl relative overflow-hidden">
              <div className="absolute top-0 right-0 w-64 h-64 bg-emerald-500/10 rounded-full blur-3xl -translate-y-1/2 translate-x-1/2"></div>

              <div className="flex flex-col md:flex-row gap-12 relative z-10">
                <div className="flex-1">
                  <div className="inline-flex items-center gap-2 px-3 py-1 rounded-full bg-zinc-800 text-zinc-300 text-sm font-medium mb-6">
                    Open Source
                  </div>
                  <h4 className="text-3xl font-bold text-white mb-2">DockPanel</h4>
                  <p className="text-zinc-400 mb-8">Self-hosted server management panel</p>

                  <div className="flex items-baseline gap-2 mb-8">
                    <span className="text-6xl font-extrabold text-white">$0</span>
                    <span className="text-xl text-zinc-500 font-medium">forever</span>
                  </div>

                  <div className="flex flex-col gap-4">
                    <a href="https://github.com/ovexro/dockpanel" className="w-full flex items-center justify-center gap-2 bg-emerald-500 hover:bg-emerald-600 text-zinc-950 px-6 py-4 rounded-xl font-bold text-lg transition-colors">
                      <Github className="w-5 h-5" />
                      View on GitHub
                    </a>
                    <a href="https://demo.dockpanel.dev" className="w-full flex items-center justify-center gap-2 bg-zinc-800 hover:bg-zinc-700 text-white px-6 py-4 rounded-xl font-bold text-lg transition-colors">
                      <Play className="w-5 h-5 fill-current" />
                      Try Live Demo
                    </a>
                  </div>
                </div>

                <div className="flex-1">
                  <ul className="grid grid-cols-1 sm:grid-cols-2 gap-y-4 gap-x-6 text-sm text-zinc-300">
                    {[
                      'Unlimited servers', 'Unlimited sites', 'Free SSL certificates',
                      'Database management + SQL browser', '54 Docker app templates',
                      'Backup orchestrator (DB, volumes, encryption, verification)', 'Web terminal & file manager',
                      'Git deploy with zero-downtime', 'Nixpacks auto-build (30+ languages)',
                      'Preview environments with TTL', 'DNS management (CF + PowerDNS)',
                      'Email management (Postfix + Dovecot)', 'Uptime monitoring & alerts',
                      'Incident management + public status page',
                      'Secrets manager (AES-256-GCM vaults)',
                      'Webhook gateway (HMAC verification)',
                      '6 themes + 3 layouts',
                      'Security scanning & hardening', '2FA / TOTP + OAuth',
                      'Auto-healing engine', 'Smart diagnostics', 'Developer CLI',
                      'Multi-server management', 'Reseller accounts & white-label',
                      'WordPress Toolkit', 'Migration wizard', 'Traefik reverse proxy option',
                      'Extension / plugin API', 'Teams & role-based access',
                      'Activity audit log', 'ARM64 / homelab support'
                    ].map((item, i) => (
                      <li key={i} className="flex items-start gap-3">
                        <Check className="w-5 h-5 text-emerald-500 shrink-0" />
                        <span>{item}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              </div>
            </div>

            <div className="mt-12 text-center">
              <h4 className="text-xl font-bold text-white mb-2">Need professional support?</h4>
              <p className="text-zinc-400 mb-4">
                DockPanel is free to use, but if you want priority support, installation help, or custom feature development, we offer optional paid support plans.
              </p>
              <a href="mailto:hello@dockpanel.dev" className="text-emerald-400 font-medium hover:text-emerald-300 transition-colors inline-flex items-center gap-1">
                Contact us at hello@dockpanel.dev
              </a>
            </div>
          </div>
        </div>
      </section>

      {/* FAQ */}
      <section id="faq" className="py-24 bg-zinc-900/30 border-t border-zinc-800/50">
        <div className="max-w-3xl mx-auto px-4 sm:px-6 lg:px-8">
          <div className="text-center mb-16">
            <h2 className="text-3xl md:text-5xl font-bold text-white tracking-tight mb-4">Frequently asked questions</h2>
          </div>

          <div className="space-y-4">
            {faqs.map((faq, i) => (
              <div key={i} className="border border-zinc-800 rounded-lg bg-zinc-900/50 overflow-hidden">
                <button
                  onClick={() => setOpenFaq(openFaq === i ? null : i)}
                  className="w-full flex items-center justify-between p-6 text-left focus:outline-none"
                >
                  <span className="text-lg font-medium text-white">{faq.q}</span>
                  <ChevronDown className={`w-5 h-5 text-zinc-500 transition-transform duration-200 ${openFaq === i ? 'rotate-180' : ''}`} />
                </button>
                <AnimatePresence>
                  {openFaq === i && (
                    <motion.div
                      initial={{ height: 0, opacity: 0 }}
                      animate={{ height: 'auto', opacity: 1 }}
                      exit={{ height: 0, opacity: 0 }}
                      transition={{ duration: 0.2 }}
                    >
                      <div className="px-6 pb-6 text-zinc-400 leading-relaxed">
                        {faq.a}
                      </div>
                    </motion.div>
                  )}
                </AnimatePresence>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* Footer */}
      <footer className="border-t border-zinc-800 bg-zinc-950 pt-16 pb-8">
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
          <div className="grid grid-cols-1 md:grid-cols-4 gap-12 mb-16">
            <div className="col-span-1 md:col-span-1">
              <Link to="/" className="flex items-center gap-2 mb-4">
                <div className="w-8 h-8 rounded-lg bg-emerald-500 flex items-center justify-center">
                  <Server className="w-5 h-5 text-zinc-950" />
                </div>
                <span className="text-xl font-bold text-white tracking-tight">DockPanel</span>
              </Link>
              <p className="text-zinc-400 text-sm leading-relaxed">
                Free, self-hosted, Docker-native server management panel built in Rust.
              </p>
            </div>

            <div>
              <h4 className="text-white font-semibold mb-4">Product</h4>
              <ul className="space-y-3 text-sm text-zinc-400">
                <li><a href="#features" className="hover:text-white transition-colors">Features</a></li>
                <li><a href="#pricing" className="hover:text-white transition-colors">Pricing</a></li>
                <li><a href="#faq" className="hover:text-white transition-colors">FAQ</a></li>
                <li><a href="https://demo.dockpanel.dev" className="hover:text-white transition-colors">Demo</a></li>
              </ul>
            </div>

            <div>
              <h4 className="text-white font-semibold mb-4">Community</h4>
              <ul className="space-y-3 text-sm text-zinc-400">
                <li><a href="https://github.com/ovexro/dockpanel" className="hover:text-white transition-colors">GitHub</a></li>
                <li><a href="https://github.com/ovexro/dockpanel#readme" className="hover:text-white transition-colors">Docs</a></li>
                <li><a href="https://github.com/ovexro/dockpanel/releases" className="hover:text-white transition-colors">Changelog</a></li>
              </ul>
            </div>

            <div>
              <h4 className="text-white font-semibold mb-4">Legal</h4>
              <ul className="space-y-3 text-sm text-zinc-400">
                <li><Link to="/privacy" className="hover:text-white transition-colors">Privacy Policy</Link></li>
                <li><Link to="/terms" className="hover:text-white transition-colors">Terms of Service</Link></li>
              </ul>
            </div>
          </div>

          <div className="border-t border-zinc-800 pt-8 flex flex-col md:flex-row items-center justify-between gap-4">
            <p className="text-zinc-500 text-sm">
              &copy; 2026 DockPanel. Free &amp; open source under the MIT License.
            </p>
          </div>
        </div>
      </footer>
    </div>
  );
}
