import { useState, useEffect, useRef } from 'react';
import { motion, AnimatePresence, useInView, useSpring, useMotionValue } from 'motion/react';
import { Link } from 'react-router-dom';
import {
  Terminal, Server, ShieldCheck, Database, Globe, Lock, Box,
  FileCode2, Activity, Stethoscope, HardDriveDownload, Cpu,
  KeyRound, Wrench, ClipboardList, Mail, Zap, Users, LogIn,
  Package, ArrowRightLeft, LayoutTemplate, Puzzle, Network,
  ChevronDown, Check, Github, Copy, CheckCircle2, X as XIcon,
  Palette, Star, ArrowRight, Menu, ScanSearch, BadgeCheck
} from 'lucide-react';

const hd = "font-['Space_Grotesk',system-ui,sans-serif]";

/* ── Animated counter ─────────────────────────────────────────────── */
function Counter({ value, suffix = '', delay = 0 }: { value: number; suffix?: string; delay?: number }) {
  const ref = useRef(null);
  const inView = useInView(ref, { once: true, margin: '-40px' });
  const motionVal = useMotionValue(0);
  const spring = useSpring(motionVal, { stiffness: 50, damping: 20 });
  const [display, setDisplay] = useState('0');

  useEffect(() => {
    if (!inView) return;
    if (delay > 0) {
      const t = setTimeout(() => motionVal.set(value), delay * 1000);
      return () => clearTimeout(t);
    }
    motionVal.set(value);
  }, [inView, value, motionVal, delay]);

  useEffect(() => {
    const unsubscribe = spring.on('change', (v: number) => setDisplay(Math.round(v).toString()));
    return unsubscribe;
  }, [spring]);

  return <span ref={ref}>{display}{suffix}</span>;
}

/* ── Animated terminal ────────────────────────────────────────────── */
function AnimatedTerminal() {
  const ref = useRef<HTMLDivElement>(null);
  const inView = useInView(ref, { once: true });
  const started = useRef(false);
  const [lines, setLines] = useState<{ text: string; cls: string }[]>([]);
  const [typing, setTyping] = useState('');
  const [phase, setPhase] = useState<'idle' | 'typing' | 'running' | 'done'>('idle');

  const command = 'curl -sL dockpanel.dev/install.sh | sudo bash';

  useEffect(() => {
    if (!inView || started.current) return;
    started.current = true;
    setPhase('typing');

    const timers: ReturnType<typeof setTimeout>[] = [];
    let i = 0;

    const typeNext = () => {
      if (i <= command.length) {
        setTyping(command.slice(0, i));
        i++;
        timers.push(setTimeout(typeNext, 25 + Math.random() * 35));
      } else {
        setPhase('running');
        setTyping('');
        setLines([{ text: '$ ' + command, cls: 'text-zinc-300' }]);

        const output: { text: string; cls: string; delay: number }[] = [
          { text: '  Detecting OS... Ubuntu 24.04 LTS', cls: 'text-zinc-600', delay: 400 },
          { text: '  Downloading dockpanel-api (41 MB)...', cls: 'text-zinc-600', delay: 600 },
          { text: '  Downloading dockpanel-agent...', cls: 'text-zinc-600', delay: 350 },
          { text: '  Configuring nginx & PostgreSQL...', cls: 'text-zinc-600', delay: 500 },
          { text: '  Starting services...', cls: 'text-zinc-600', delay: 400 },
          { text: '\u00A0', cls: '', delay: 250 },
          { text: '\u2713 DockPanel v2.7.4 installed in 47s', cls: 'text-emerald-400 font-medium', delay: 350 },
          { text: '  Panel \u2192 https://your-server:3080', cls: 'text-zinc-300', delay: 200 },
        ];

        let cum = 500;
        output.forEach((line) => {
          cum += line.delay;
          timers.push(setTimeout(() => {
            setLines(prev => [...prev, line]);
            if (line.text.startsWith('\u2713')) setPhase('done');
          }, cum));
        });
      }
    };

    timers.push(setTimeout(typeNext, 600));
    return () => timers.forEach(clearTimeout);
  }, [inView]);

  return (
    <div ref={ref} className="w-full max-w-2xl mx-auto rounded-xl overflow-hidden border border-zinc-800 bg-zinc-950 shadow-2xl shadow-black/50">
      <div className="h-9 bg-zinc-900 border-b border-zinc-800 flex items-center px-3.5 gap-2">
        <div className="w-2.5 h-2.5 rounded-full bg-[#ff5f57]" />
        <div className="w-2.5 h-2.5 rounded-full bg-[#febc2e]" />
        <div className="w-2.5 h-2.5 rounded-full bg-[#28c840]" />
        <span className="flex-1 text-center text-[11px] text-zinc-600 font-medium select-none">Terminal</span>
      </div>
      <div className="p-4 sm:p-5 font-mono text-[13px] leading-relaxed min-h-[220px]">
        {phase === 'typing' && (
          <div>
            <span className="text-zinc-600">$ </span>
            <span className="text-zinc-300">{typing}</span>
            <span className="terminal-cursor">{'\u2588'}</span>
          </div>
        )}
        {lines.map((line, i) => (
          <div key={i} className={line.cls}>{line.text}</div>
        ))}
        {phase === 'running' && <span className="terminal-cursor">{'\u2588'}</span>}
      </div>
    </div>
  );
}

/* ── RAM comparison bar ───────────────────────────────────────────── */
function RamBar({ name, mb, max, highlight = false, delay = 0 }: {
  name: string; mb: number; max: number; highlight?: boolean; delay?: number;
}) {
  const ref = useRef(null);
  const inView = useInView(ref, { once: true, margin: '-20px' });
  const width = (mb / max) * 100;

  return (
    <div ref={ref} className="flex items-center gap-4">
      <span className={`w-24 text-right text-sm font-medium shrink-0 ${highlight ? 'text-white' : 'text-zinc-500'}`}>
        {name}
      </span>
      <div className="flex-1 h-8 bg-zinc-900 rounded-md overflow-hidden border border-zinc-800">
        <motion.div
          initial={{ width: 0 }}
          animate={inView ? { width: `${width}%` } : { width: 0 }}
          transition={{ duration: 0.8, delay, ease: [0.23, 1, 0.32, 1] }}
          className={`h-full rounded-md flex items-center px-3 ${highlight
            ? 'bg-white'
            : 'bg-zinc-800'
          }`}
        >
          <span className={`text-xs font-mono font-medium whitespace-nowrap ${highlight ? 'text-zinc-900' : 'text-zinc-400'}`}>
            ~{mb} MB
          </span>
        </motion.div>
      </div>
    </div>
  );
}

/* ── Nav link with active indicator ───────────────────────────────── */
function NavLink({ href, label, active }: { href: string; label: string; active: boolean }) {
  return (
    <a href={href} className={`relative hover:text-white transition-colors pb-1 ${active ? 'text-white' : ''}`}>
      {label}
      <span className={`absolute bottom-0 left-0 right-0 h-px bg-white rounded-full transition-opacity duration-300 ${active ? 'opacity-100' : 'opacity-0'}`} />
    </a>
  );
}

/* ── Data ─────────────────────────────────────────────────────────── */
const showcase = [
  {
    title: '151 templates. One click.',
    desc: 'WordPress, Postgres, Grafana, n8n, Immich \u2014 pick a template. SSL, reverse proxy, and networking are configured automatically.',
    shot: '/screenshots/dp-apps.png',
    alt: 'Docker Apps gallery',
  },
  {
    title: 'Monitoring that wakes you up.',
    desc: 'HTTP, TCP, and DNS probes. Incident timelines. Public status pages. Alerts to Slack, Discord, or PagerDuty.',
    shot: '/screenshots/dp-monitoring.png',
    alt: 'Monitoring dashboard',
  },
  {
    title: 'Seven audits. 280 vulns fixed.',
    desc: 'ModSecurity WAF, Fail2Ban, per-image CVE scanning with deploy gating, and one-click hardening. Security is the default, not an add-on.',
    shot: '/screenshots/dp-security.png',
    alt: 'Security dashboard',
  },
  {
    title: 'Git push \u2192 production.',
    desc: 'Atomic deploys with symlink swap. Preview every branch. Roll back in one click when Friday deploys go sideways.',
    shot: '/screenshots/dp-git-deploy.png',
    alt: 'Git deploy',
  },
];

const allFeatures = [
  { name: 'Site Management', desc: 'PHP, Node, Python, static & reverse proxy', icon: Globe },
  { name: 'Free SSL & Wildcard', desc: "Auto-renew via Let's Encrypt & Cloudflare DNS", icon: Lock },
  { name: 'MySQL & PostgreSQL', desc: 'Create, backup, restore, point-in-time recovery', icon: Database },
  { name: '151 Docker Templates', desc: 'One-click deploy across 14 categories', icon: Box },
  { name: 'Web Terminal', desc: 'Full PTY with privilege drop & command filtering', icon: Terminal },
  { name: 'Infrastructure as Code', desc: 'YAML export/import for reproducible setups', icon: FileCode2 },
  { name: 'Uptime Monitoring', desc: 'HTTP, TCP, ping probes with incident management', icon: Activity },
  { name: 'WAF (ModSecurity)', desc: 'OWASP CRS v4 \u2014 per-site detection or prevention', icon: ShieldCheck },
  { name: 'Image CVE Scanning', desc: 'Per-app grype scans, deploy gate, scheduled rescan', icon: ScanSearch },
  { name: 'Signed Releases + SBOM', desc: 'cosign keyless via Sigstore, SPDX 2.3 SBOMs, Rekor transparency log', icon: BadgeCheck },
  { name: 'Per-Image SBOM', desc: 'On-demand SPDX 2.3 download per deployed container (syft)', icon: Package },
  { name: 'Backup Orchestrator', desc: 'Scheduled, encrypted, verified, with S3 support', icon: HardDriveDownload },
  { name: 'Smart Diagnostics', desc: '6 check categories with one-click auto-fix', icon: Stethoscope },
  { name: 'GPU Passthrough', desc: 'NVIDIA Container Toolkit detection & monitoring', icon: Cpu },
  { name: 'Passkeys & 2FA', desc: 'WebAuthn, TOTP, recovery codes', icon: KeyRound },
  { name: 'Auto-Healing', desc: 'Restart services, clear disk, kill runaway processes', icon: Wrench },
  { name: 'Audit Log', desc: 'Immutable, tamper-resistant, with session recording', icon: ClipboardList },
  { name: 'Email Server', desc: 'Postfix + Dovecot + DKIM, virtual domains & aliases', icon: Mail },
  { name: 'Reseller & White-Label', desc: 'Custom branding, teams, billing integration', icon: Users },
  { name: 'Multi-Server', desc: 'Manage a fleet from one dashboard', icon: Server },
  { name: 'OAuth / SSO', desc: 'GitHub & Google login out of the box', icon: LogIn },
  { name: 'Nixpacks Build', desc: '30+ languages without writing a Dockerfile', icon: Package },
  { name: 'Migration Wizard', desc: 'Import from cPanel, Hestia, or WordPress', icon: ArrowRightLeft },
  { name: 'WordPress Toolkit', desc: 'Bulk scanning, hardening, safe updates', icon: Puzzle },
  { name: 'Webhook Gateway', desc: 'Receive, inspect, route, and replay webhooks', icon: Zap },
  { name: 'Traefik Proxy', desc: 'Install, manage routes, automatic discovery', icon: Network },
  { name: '6 Themes, 3 Layouts', desc: 'Terminal, midnight, ember, arctic, and more', icon: Palette },
  { name: 'CDN Integration', desc: 'BunnyCDN & Cloudflare management, cache purge', icon: Globe },
  { name: 'Terraform / Pulumi', desc: 'IaC provider for external automation', icon: LayoutTemplate },
];

const steps = [
  { icon: Terminal, title: 'Install', desc: 'One command. Under 60 seconds. No dependencies to manage.' },
  { icon: Globe, title: 'Configure', desc: 'Add sites, databases, and Docker apps from the dashboard.' },
  { icon: Zap, title: 'Deploy', desc: 'Push code, manage containers, monitor everything in real time.' },
];

const faqs = [
  { q: 'Is it really free?', a: 'Every feature, every server, no limits. Licensed under BSL 1.1, which converts to MIT in 2030. There is no premium tier.' },
  { q: 'System requirements?', a: '512 MB RAM, 1 CPU, 10 GB disk. Runs on Ubuntu, Debian, CentOS, Rocky, and Amazon Linux. ARM64 works too.' },
  { q: 'How is this different from cPanel?', a: "cPanel uses ~800 MB of RAM, costs $15/month, and doesn't support Docker. DockPanel's panel services idle around ~19 MB (about ~85 MB with the bundled PostgreSQL), cost nothing, and ship with 151 Docker templates, a WAF, passkey authentication, Git deploys, a CLI, and multi-server management." },
  { q: 'What happens if DockPanel goes down?', a: 'Your sites keep running. Nginx and Docker are independent processes \u2014 the panel is just the management layer. It auto-restarts via systemd if it ever stops.' },
  { q: 'Can I manage multiple servers?', a: 'As many as you want. Install a lightweight agent on each server and manage them all from one dashboard.' },
  { q: 'Why Rust?', a: "~41 MB of binaries on disk, ~19 MB of RAM for the panel services at idle (measured on a fresh Vultr VPS), no JVM, no Node, no Python dependency to maintain. On a $5 VPS, that's the difference between running 20 sites and running 2." },
];

/* ── Page ─────────────────────────────────────────────────────────── */
export default function Landing() {
  const [copied, setCopied] = useState(false);
  const [openFaq, setOpenFaq] = useState<number | null>(null);
  const [stars, setStars] = useState<number | null>(null);
  const [lightbox, setLightbox] = useState<{ src: string; alt: string } | null>(null);
  const [scrolled, setScrolled] = useState(false);
  const [mobileMenu, setMobileMenu] = useState(false);
  const [activeSection, setActiveSection] = useState('');

  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 20);
    window.addEventListener('scroll', onScroll, { passive: true });
    return () => window.removeEventListener('scroll', onScroll);
  }, []);

  /* active nav tracking */
  useEffect(() => {
    const ids = ['features', 'compare', 'pricing', 'faq'];
    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach(entry => {
          if (entry.isIntersecting) setActiveSection(entry.target.id);
        });
      },
      { threshold: 0.15, rootMargin: '-80px 0px -50% 0px' }
    );
    ids.forEach(id => {
      const el = document.getElementById(id);
      if (el) observer.observe(el);
    });
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    fetch('https://api.github.com/repos/ovexro/dockpanel')
      .then(r => r.json())
      .then(d => { if (d.stargazers_count) setStars(d.stargazers_count); })
      .catch(() => {});
  }, []);

  const handleCopy = () => {
    navigator.clipboard.writeText('curl -sL https://dockpanel.dev/install.sh | sudo bash');
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="noise min-h-screen bg-[#09090b] text-zinc-400 selection:bg-white/15 selection:text-white overflow-x-hidden">

      {/* ── Lightbox ─────────────────────────────────────────────── */}
      <AnimatePresence>
        {lightbox && (
          <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}
            className="fixed inset-0 z-[100] flex items-center justify-center p-6 sm:p-12 cursor-zoom-out"
            onClick={() => setLightbox(null)}
          >
            <div className="absolute inset-0 bg-black/95 backdrop-blur-md" />
            <motion.img
              initial={{ scale: 0.92, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} exit={{ scale: 0.92, opacity: 0 }}
              transition={{ duration: 0.25, ease: [0.23, 1, 0.32, 1] }}
              src={lightbox.src} alt={lightbox.alt}
              className="relative max-w-full max-h-full rounded-lg shadow-2xl"
            />
            <button className="absolute top-6 right-6 p-2 text-zinc-500 hover:text-white transition-colors">
              <XIcon className="w-6 h-6" />
            </button>
          </motion.div>
        )}
      </AnimatePresence>

      {/* ── Nav ───────────────────────────────────────────────────── */}
      <nav className={`fixed top-0 left-0 right-0 z-50 transition-all duration-300 ${scrolled ? 'bg-[#09090b]/80 backdrop-blur-2xl border-b border-zinc-800/60' : 'border-b border-transparent'}`}>
        <div className="max-w-7xl mx-auto px-6 h-14 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2.5">
            <div className="w-7 h-7 rounded-md bg-white flex items-center justify-center">
              <Server className="w-4 h-4 text-zinc-900" />
            </div>
            <span className={`text-lg font-bold text-white ${hd}`}>DockPanel</span>
          </Link>
          <div className="hidden md:flex items-center gap-8 text-[13px] font-medium text-zinc-500">
            <NavLink href="#features" label="Features" active={activeSection === 'features'} />
            <NavLink href="#compare" label="Compare" active={activeSection === 'compare'} />
            <NavLink href="#pricing" label="Pricing" active={activeSection === 'pricing'} />
            <NavLink href="#faq" label="FAQ" active={activeSection === 'faq'} />
          </div>
          <div className="flex items-center gap-3">
            <a href="https://github.com/ovexro/dockpanel" className="hidden sm:flex items-center gap-1.5 text-[13px] text-zinc-500 hover:text-white transition-colors">
              <Github className="w-4 h-4" />
              {stars !== null && (
                <span className="flex items-center gap-1 text-xs">
                  <Star className="w-3 h-3 text-zinc-400 fill-zinc-400" />
                  {stars >= 1000 ? `${(stars / 1000).toFixed(1)}k` : stars}
                </span>
              )}
            </a>
            <a href="https://docs.dockpanel.dev" className="hidden sm:block text-[13px] font-semibold px-4 py-1.5 rounded-md bg-white text-zinc-900 hover:bg-zinc-200 transition-colors">
              Docs
            </a>
            <button onClick={() => setMobileMenu(true)} className="md:hidden p-1 text-zinc-400 hover:text-white transition-colors">
              <Menu className="w-5 h-5" />
            </button>
          </div>
        </div>
      </nav>

      {/* ── Mobile menu ──────────────────────────────────────────── */}
      <AnimatePresence>
        {mobileMenu && (
          <>
            <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}
              className="fixed inset-0 z-[60] bg-black/60 backdrop-blur-sm md:hidden"
              onClick={() => setMobileMenu(false)}
            />
            <motion.div
              initial={{ x: '100%' }} animate={{ x: 0 }} exit={{ x: '100%' }}
              transition={{ type: 'spring', damping: 30, stiffness: 300 }}
              className="fixed top-0 right-0 bottom-0 w-64 z-[70] bg-zinc-900 border-l border-zinc-800 p-6 md:hidden"
            >
              <button onClick={() => setMobileMenu(false)} className="absolute top-4 right-4 p-1 text-zinc-500 hover:text-white transition-colors">
                <XIcon className="w-5 h-5" />
              </button>
              <div className="flex flex-col gap-6 mt-12 text-[15px] font-medium">
                <a href="#features" onClick={() => setMobileMenu(false)} className="text-zinc-300 hover:text-white transition-colors">Features</a>
                <a href="#compare" onClick={() => setMobileMenu(false)} className="text-zinc-300 hover:text-white transition-colors">Compare</a>
                <a href="#pricing" onClick={() => setMobileMenu(false)} className="text-zinc-300 hover:text-white transition-colors">Pricing</a>
                <a href="#faq" onClick={() => setMobileMenu(false)} className="text-zinc-300 hover:text-white transition-colors">FAQ</a>
                <hr className="border-zinc-800" />
                <a href="https://github.com/ovexro/dockpanel" className="flex items-center gap-2 text-zinc-300 hover:text-white transition-colors">
                  <Github className="w-4 h-4" /> GitHub
                </a>
                <a href="https://docs.dockpanel.dev" className="flex items-center gap-2 text-zinc-300 hover:text-white transition-colors">
                  Docs
                </a>
              </div>
            </motion.div>
          </>
        )}
      </AnimatePresence>

      {/* ── Hero ──────────────────────────────────────────────────── */}
      <section className="relative pt-28 sm:pt-36 lg:pt-44 pb-4 overflow-hidden hero-grid">
        {/* top vignette */}
        <div className="absolute inset-0 bg-gradient-to-b from-[#09090b] via-transparent to-[#09090b] pointer-events-none" />

        <div className="max-w-7xl mx-auto px-6 relative z-10">
          <motion.div initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.6 }}
            className="text-center max-w-3xl mx-auto"
          >
            <div className="flex items-center justify-center gap-2 mb-6">
              <span className="inline-flex items-center px-3 py-1 text-[11px] font-medium text-zinc-400 bg-zinc-800/50 border border-zinc-800 rounded-full">
                Open source
              </span>
              <span className="inline-flex items-center px-3 py-1 text-[11px] font-medium text-zinc-500 bg-zinc-900/50 border border-zinc-800/50 rounded-full">
                Written in Rust
              </span>
            </div>

            <h1 className={`text-[3.5rem] sm:text-[5rem] lg:text-[5.5rem] font-bold text-white leading-[1.02] tracking-[-0.04em] mb-7 ${hd}`}>
              Server management.<br />
              <span className="text-zinc-500"><Counter value={57} delay={0.5} />&nbsp;megabytes.</span>
            </h1>

            <p className="text-lg sm:text-xl text-zinc-400 leading-relaxed mb-10 max-w-2xl mx-auto">
              Sites, Docker apps, databases, email, monitoring, security, backups, and Git deploys &mdash; one install, one binary, <span className="text-white font-medium">zero cost.</span>
            </p>

            <div className="mb-7 flex flex-col items-center">
              <div className="install-glow inline-flex items-center bg-zinc-900 border border-zinc-800 rounded-lg p-1 hover:border-zinc-700 transition-colors">
                <span className="px-3 text-zinc-600 font-mono text-sm select-none">$</span>
                <code className="text-zinc-300 font-mono text-sm pr-3">
                  curl -sL dockpanel.dev/install.sh | sudo bash
                </code>
                <button onClick={handleCopy} className="px-3 py-2 bg-zinc-800 hover:bg-zinc-700 rounded-md text-xs text-zinc-300 font-medium transition-colors flex items-center gap-1.5">
                  {copied ? <Check className="w-3.5 h-3.5 text-emerald-400" /> : <Copy className="w-3.5 h-3.5" />}
                  {copied ? 'Copied' : 'Copy'}
                </button>
              </div>
              <p className="text-[11px] text-zinc-600 mt-2.5 tracking-wide font-medium">Ubuntu &middot; Debian &middot; CentOS &middot; Rocky &middot; Amazon Linux &middot; ARM64</p>
            </div>

            <div className="flex items-center justify-center gap-3">
              <a href="https://docs.dockpanel.dev" className="flex items-center gap-2 bg-white hover:bg-zinc-200 text-zinc-900 px-6 py-3 rounded-lg text-[15px] font-bold transition-colors">
                Get Started <ArrowRight className="w-4 h-4" />
              </a>
              <a href="https://github.com/ovexro/dockpanel" className="flex items-center gap-2 text-[15px] font-medium text-zinc-300 hover:text-white px-5 py-3 rounded-lg border border-zinc-800 hover:border-zinc-700 bg-zinc-900/50 transition-all">
                <Github className="w-4 h-4" /> Source
              </a>
            </div>
          </motion.div>

          {/* Terminal */}
          <motion.div initial={{ opacity: 0, y: 30 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.7, delay: 0.2 }}
            className="mt-14"
          >
            <AnimatedTerminal />
          </motion.div>

          {/* Dashboard screenshot */}
          <motion.div
            initial={{ opacity: 0, y: 40 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.8, delay: 0.4 }}
            className="mt-10 relative"
          >
            <div className="max-w-5xl mx-auto rounded-xl overflow-hidden border border-zinc-800/60 shadow-2xl shadow-black/60">
              <div className="h-8 bg-zinc-900 border-b border-zinc-800/60 flex items-center px-3.5 gap-2">
                <div className="w-2.5 h-2.5 rounded-full bg-[#ff5f57]" />
                <div className="w-2.5 h-2.5 rounded-full bg-[#febc2e]" />
                <div className="w-2.5 h-2.5 rounded-full bg-[#28c840]" />
              </div>
              <img src="/screenshots/dp-dashboard.png" alt="DockPanel Dashboard" className="w-full block" />
            </div>
            <div className="absolute bottom-0 left-0 right-0 h-40 bg-gradient-to-t from-[#09090b] to-transparent pointer-events-none" />
          </motion.div>
        </div>
      </section>

      {/* ── Numbers ──────────────────────────────────────────────── */}
      <section className="border-y border-zinc-800/60 bg-zinc-950/50 glow-divider">
        <div className="max-w-7xl mx-auto px-6 py-8">
          <div className="flex flex-wrap items-center justify-between gap-y-4 gap-x-4">
            {[
              { v: 41, s: '~', e: 'MB', l: 'binary' },
              { v: 57, s: '~', e: 'MB', l: 'RAM' },
              { v: 60, s: '<', e: 's', l: 'install' },
              { v: 151, s: '', e: '', l: 'templates' },
              { v: 26, s: '', e: '', l: 'modules' },
              { v: 6, s: '', e: '', l: 'security audits' },
            ].map((s, i) => (
              <div key={i} className="flex items-baseline gap-1.5">
                <span className={`text-2xl font-bold text-white tabular-nums ${hd}`}>
                  {s.s}<Counter value={s.v} />{s.e}
                </span>
                <span className="text-[11px] text-zinc-600 font-medium">{s.l}</span>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* ── How it works ─────────────────────────────────────────── */}
      <section className="py-20 lg:py-28">
        <div className="max-w-4xl mx-auto px-6">
          <motion.div initial={{ opacity: 0, y: 15 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true }}
            className="text-center mb-14"
          >
            <h2 className={`text-[2rem] sm:text-[2.5rem] font-bold text-white leading-tight tracking-tight ${hd}`}>
              How it works.
            </h2>
          </motion.div>

          <div className="grid md:grid-cols-3 gap-6 md:gap-8">
            {steps.map((step, i) => (
              <motion.div key={i}
                initial={{ opacity: 0, y: 15 }}
                whileInView={{ opacity: 1, y: 0 }}
                viewport={{ once: true }}
                transition={{ duration: 0.4, delay: i * 0.15 }}
                className="text-center"
              >
                <div className="inline-flex items-center justify-center w-12 h-12 rounded-xl bg-zinc-900 border border-zinc-800 mb-4">
                  <step.icon className="w-5 h-5 text-zinc-300" />
                </div>
                <div className="text-[11px] font-bold text-zinc-600 uppercase tracking-widest mb-2">Step {i + 1}</div>
                <h3 className={`text-xl font-bold text-white mb-2 ${hd}`}>{step.title}</h3>
                <p className="text-[14px] text-zinc-500 leading-relaxed">{step.desc}</p>
              </motion.div>
            ))}
          </div>
        </div>
      </section>

      {/* ── Feature showcase (bento grid) ────────────────────────── */}
      <section id="features" className="py-28 lg:py-36 border-t border-zinc-800/60 glow-divider">
        <div className="max-w-7xl mx-auto px-6">
          <motion.div initial={{ opacity: 0, y: 15 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true }}
            className="text-center mb-14"
          >
            <h2 className={`text-[2.5rem] sm:text-[3rem] font-bold text-white leading-tight tracking-tight ${hd}`}>
              What you get.
            </h2>
            <p className="text-zinc-500 text-[15px] mt-3">Four pillars. Hundreds of features. One binary.</p>
          </motion.div>

          <div className="grid md:grid-cols-2 gap-4">
            {showcase.map((feat, i) => (
              <motion.div key={i}
                initial={{ opacity: 0, y: 20 }}
                whileInView={{ opacity: 1, y: 0 }}
                viewport={{ once: true, margin: '-40px' }}
                transition={{ duration: 0.5, delay: i * 0.1 }}
                className="group rounded-2xl border border-zinc-800/60 bg-zinc-900/30 p-6 hover:border-zinc-600/50 hover:shadow-[0_0_40px_-12px_rgba(255,255,255,0.06)] transition-all duration-500"
              >
                <h3 className={`text-xl sm:text-2xl font-bold text-white mb-2 ${hd}`}>{feat.title}</h3>
                <p className="text-[15px] text-zinc-500 leading-relaxed mb-5">{feat.desc}</p>
                <div className="cursor-pointer overflow-hidden rounded-lg" onClick={() => setLightbox({ src: feat.shot, alt: feat.alt })}>
                  <div className="rounded-lg overflow-hidden border border-zinc-800/60 shadow-lg shadow-black/40 transition-transform duration-500 group-hover:scale-[1.02]">
                    <div className="h-7 bg-zinc-900 border-b border-zinc-800/60 flex items-center px-3 gap-1.5">
                      <div className="w-2 h-2 rounded-full bg-[#ff5f57]" />
                      <div className="w-2 h-2 rounded-full bg-[#febc2e]" />
                      <div className="w-2 h-2 rounded-full bg-[#28c840]" />
                    </div>
                    <img src={feat.shot} alt={feat.alt} className="w-full block transition-[filter] duration-500 group-hover:brightness-110" loading="lazy" />
                  </div>
                </div>
              </motion.div>
            ))}
          </div>
        </div>
      </section>

      {/* ── All features (compact grid, staggered) ───────────────── */}
      <section className="py-20 lg:py-28 border-y border-zinc-800/60 bg-zinc-950/50 glow-divider">
        <div className="max-w-6xl mx-auto px-6">
          <div className="text-center mb-12">
            <h2 className={`text-2xl font-bold text-white ${hd}`}>Everything ships included.</h2>
          </div>

          <div className="grid sm:grid-cols-2 lg:grid-cols-3 gap-3">
            {allFeatures.map(({ name, desc, icon: Icon }, i) => (
              <motion.div key={i}
                initial={{ opacity: 0, y: 10 }}
                whileInView={{ opacity: 1, y: 0 }}
                viewport={{ once: true, margin: '-10px' }}
                transition={{ duration: 0.3, delay: Math.floor(i / 3) * 0.06 }}
                className="flex items-start gap-3 p-4 rounded-xl border border-zinc-800/40 bg-zinc-900/20 hover:border-zinc-600/40 hover:shadow-[0_0_30px_-10px_rgba(255,255,255,0.04)] transition-all duration-300"
              >
                <div className="w-8 h-8 rounded-lg bg-zinc-800/50 flex items-center justify-center shrink-0 mt-0.5">
                  <Icon className="w-4 h-4 text-zinc-500" />
                </div>
                <div className="min-w-0">
                  <span className="text-[13px] font-semibold text-zinc-200 block">{name}</span>
                  <p className="text-[12px] text-zinc-600 leading-relaxed mt-0.5">{desc}</p>
                </div>
              </motion.div>
            ))}
          </div>
        </div>
      </section>

      {/* ── Visual comparison ────────────────────────────────────── */}
      <section id="compare" className="py-28 lg:py-36">
        <div className="max-w-4xl mx-auto px-6">
          <motion.div initial={{ opacity: 0, y: 15 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true }}
            className="text-center mb-12"
          >
            <h2 className={`text-[2.5rem] sm:text-[3rem] font-bold text-white mb-3 leading-tight tracking-tight ${hd}`}>
              The numbers.
            </h2>
            <p className="text-zinc-500 text-[15px]">Memory usage at idle. Less RAM means more room for your apps.</p>
          </motion.div>

          <div className="space-y-3 mb-16">
            <RamBar name="DockPanel" mb={19} max={800} highlight delay={0} />
            <RamBar name="CloudPanel" mb={250} max={800} delay={0.1} />
            <RamBar name="Plesk" mb={512} max={800} delay={0.2} />
            <RamBar name="HestiaCP" mb={512} max={800} delay={0.3} />
            <RamBar name="cPanel" mb={800} max={800} delay={0.4} />
          </div>

          <motion.div initial={{ opacity: 0, y: 15 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true }}
            className="overflow-x-auto rounded-xl border border-zinc-800/60 bg-zinc-900/30"
          >
            <table className="w-full text-left min-w-[580px]">
              <thead>
                <tr className="border-b border-zinc-800/60">
                  {['', 'Install', 'Price', 'Docker', 'Self-hosted'].map(h => (
                    <th key={h} className={`px-5 py-4 text-[10px] font-bold text-zinc-500 uppercase tracking-widest ${h === 'Docker' || h === 'Self-hosted' ? 'text-center' : ''}`}>{h}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {[
                  { n: 'DockPanel', t: '<60 sec', p: 'Free', d: true, s: true, hl: true },
                  { n: 'cPanel', t: '~45 min', p: '$15/mo', d: false, s: true },
                  { n: 'Plesk', t: '~20 min', p: '$10/mo', d: false, s: true },
                  { n: 'RunCloud', t: '~5 min', p: '$8/mo', d: false, s: false },
                  { n: 'CloudPanel', t: '~10 min', p: 'Free', d: false, s: true },
                  { n: 'HestiaCP', t: '~2 min', p: 'Free', d: false, s: true },
                ].map((row, i) => (
                  <tr key={i} className={`border-b border-zinc-800/30 last:border-0 transition-colors ${row.hl ? 'bg-white/[0.03]' : i % 2 === 0 ? 'bg-white/[0.01]' : ''} hover:bg-white/[0.03]`}>
                    <td className={`px-5 py-4 font-semibold text-sm ${row.hl ? 'text-white' : 'text-zinc-300'}`}>{row.n}</td>
                    <td className="px-5 py-4 text-sm">{row.t}</td>
                    <td className="px-5 py-4 text-sm">{row.p}</td>
                    <td className="px-5 py-4 text-center">{row.d ? <CheckCircle2 className="w-4 h-4 text-white mx-auto" /> : <XIcon className="w-4 h-4 text-zinc-700 mx-auto" />}</td>
                    <td className="px-5 py-4 text-center">{row.s ? <CheckCircle2 className="w-4 h-4 text-white mx-auto" /> : <XIcon className="w-4 h-4 text-zinc-700 mx-auto" />}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </motion.div>
        </div>
      </section>

      {/* ── Pricing ──────────────────────────────────────────────── */}
      <section id="pricing" className="py-32 lg:py-40 border-t border-zinc-800/60 glow-divider">
        <div className="max-w-2xl mx-auto px-6 text-center">
          <motion.div initial={{ opacity: 0, y: 15 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true }}>
            <h2 className={`text-7xl sm:text-8xl lg:text-9xl font-bold text-white mt-3 mb-3 tracking-tighter ${hd}`}>
              $0
            </h2>
            <p className="text-xl text-zinc-500 font-medium mb-6">forever</p>
            <p className="text-[17px] text-zinc-400 mb-10 leading-relaxed max-w-md mx-auto">
              Every feature. Every server. No tiers, no per-site fees, no usage limits.
              Open source under BSL 1.1, converting to MIT in 2030.
            </p>

            <div className="flex flex-col sm:flex-row items-center justify-center gap-3 mb-8">
              <a href="https://github.com/ovexro/dockpanel" className="flex items-center gap-2 bg-white hover:bg-zinc-200 text-zinc-900 px-6 py-3 rounded-lg text-sm font-bold transition-colors">
                <Github className="w-4 h-4" /> View on GitHub
              </a>
              <a href="https://docs.dockpanel.dev" className="flex items-center gap-2 border border-zinc-800 hover:border-zinc-700 bg-zinc-900/50 text-white px-6 py-3 rounded-lg text-sm font-bold transition-all">
                Read the docs <ArrowRight className="w-4 h-4" />
              </a>
            </div>

            <p className="text-[13px] text-zinc-600">
              Need help?{' '}
              <a href="mailto:hello@dockpanel.dev" className="text-zinc-400 hover:text-white transition-colors underline underline-offset-4 decoration-zinc-800 hover:decoration-zinc-600">
                hello@dockpanel.dev
              </a>
            </p>
          </motion.div>
        </div>
      </section>

      {/* ── FAQ ──────────────────────────────────────────────────── */}
      <section id="faq" className="py-24 lg:py-32 border-t border-zinc-800/60 glow-divider">
        <div className="max-w-2xl mx-auto px-6">
          <div className="text-center mb-10">
            <h2 className={`text-2xl font-bold text-white ${hd}`}>FAQ</h2>
          </div>
          <div>
            {faqs.map((faq, i) => (
              <div key={i} className="border-b border-zinc-800/40">
                <button onClick={() => setOpenFaq(openFaq === i ? null : i)}
                  className="w-full flex items-center justify-between py-5 text-left group"
                >
                  <span className="text-[15px] font-medium text-zinc-200 group-hover:text-white transition-colors">{faq.q}</span>
                  <ChevronDown className={`w-4 h-4 text-zinc-600 group-hover:text-zinc-400 transition-all duration-200 shrink-0 ml-4 ${openFaq === i ? 'rotate-180' : ''}`} />
                </button>
                <AnimatePresence>
                  {openFaq === i && (
                    <motion.div initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={{ duration: 0.15 }}>
                      <p className="pb-5 text-[14px] text-zinc-500 leading-relaxed">{faq.a}</p>
                    </motion.div>
                  )}
                </AnimatePresence>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* ── Footer ───────────────────────────────────────────────── */}
      <footer className="border-t border-zinc-800/60 pt-14 pb-10">
        <div className="max-w-7xl mx-auto px-6">
          <div className="grid md:grid-cols-[1fr,auto] gap-10 items-start">
            <div>
              <div className="flex items-center gap-2.5 mb-3">
                <div className="w-7 h-7 rounded-md bg-white flex items-center justify-center">
                  <Server className="w-4 h-4 text-zinc-900" />
                </div>
                <span className={`text-lg font-bold text-white ${hd}`}>DockPanel</span>
              </div>
              <p className="text-[13px] text-zinc-600 max-w-xs leading-relaxed">
                Lightweight, Docker-native server management.<br />
                Open source, self-hosted, zero cost.
              </p>
            </div>
            <div className="flex gap-16">
              <div>
                <h4 className="text-[11px] font-bold text-zinc-500 uppercase tracking-widest mb-3">Product</h4>
                <div className="flex flex-col gap-2.5 text-[13px] text-zinc-600">
                  <a href="#features" className="hover:text-zinc-300 transition-colors">Features</a>
                  <a href="https://docs.dockpanel.dev" className="hover:text-zinc-300 transition-colors">Docs</a>
                  <a href="https://github.com/ovexro/dockpanel" className="hover:text-zinc-300 transition-colors">GitHub</a>
                </div>
              </div>
              <div>
                <h4 className="text-[11px] font-bold text-zinc-500 uppercase tracking-widest mb-3">Legal</h4>
                <div className="flex flex-col gap-2.5 text-[13px] text-zinc-600">
                  <Link to="/privacy" className="hover:text-zinc-300 transition-colors">Privacy</Link>
                  <Link to="/terms" className="hover:text-zinc-300 transition-colors">Terms</Link>
                </div>
              </div>
            </div>
          </div>
          <div className="mt-10 pt-6 border-t border-zinc-800/40 flex flex-col sm:flex-row items-center justify-between gap-2">
            <span className="text-[12px] text-zinc-700">&copy; 2026 DockPanel</span>
            <span className="text-[11px] text-zinc-700">Solo-developed &middot; BSL 1.1</span>
          </div>
        </div>
      </footer>
    </div>
  );
}
