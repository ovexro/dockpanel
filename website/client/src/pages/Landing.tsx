import { useState, useEffect, useRef } from 'react';
import { motion, AnimatePresence, useInView, useSpring, useMotionValue } from 'motion/react';
import { Link } from 'react-router-dom';
import { Logo } from '../components/Logo';
import { measurements as m } from '../measurements';
import {
  Terminal, Server, ShieldCheck, Database, Globe, Lock, Box,
  FileCode2, Activity, Stethoscope, HardDriveDownload, Cpu,
  KeyRound, Wrench, ClipboardList, Mail, Zap, Users, LogIn,
  Package, ArrowRightLeft, LayoutTemplate, Puzzle, Network,
  ChevronDown, Check, Github, Copy, CheckCircle2, X as XIcon,
  Palette, Star, ArrowRight, Menu, ScanSearch, BadgeCheck
} from 'lucide-react';


/* ── Settling counter ─────────────────────────────────────────────────
   The one animation on this page, and its whole job is to show a
   measurement being TAKEN rather than printed: the figures arrive in order
   and each decelerates onto its value, which is what a reading settling
   looks like. It runs once and nothing loops.

   A plain rAF tween rather than motion's `useSpring`: no library timing
   semantics to reason about, and the state is observable at any instant,
   which matters for a component whose entire behaviour is temporal.      */
function Counter({ value, suffix = '', delay = 0 }: { value: number; suffix?: string; delay?: number }) {
  const ref = useRef<HTMLSpanElement>(null);
  const inView = useInView(ref, { once: true, margin: '-40px' });
  const [display, setDisplay] = useState(0);

  useEffect(() => {
    if (!inView) return;

    // A media query cannot stop a JS tween, so honour the preference here:
    // anyone who asked not to be animated gets the figure straight away.
    if (window.matchMedia?.('(prefers-reduced-motion: reduce)').matches) {
      setDisplay(value);
      return;
    }

    let raf = 0;
    let start = 0;
    const DURATION = 900;
    // Decelerating, so the number slows into place the way a reading settles
    // rather than stopping dead.
    const ease = (t: number) => 1 - Math.pow(1 - t, 3);

    const tick = (now: number) => {
      if (!start) start = now;
      const t = Math.min((now - start) / DURATION, 1);
      setDisplay(Math.round(ease(t) * value));
      if (t < 1) raf = requestAnimationFrame(tick);
    };

    const timer = setTimeout(() => { raf = requestAnimationFrame(tick); }, delay * 1000);
    return () => { clearTimeout(timer); cancelAnimationFrame(raf); };
  }, [inView, value, delay]);

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
          { text: `  Downloading dockpanel-api (${m.binary.api} MB)...`, cls: 'text-zinc-600', delay: 600 },
          { text: '  Downloading dockpanel-agent...', cls: 'text-zinc-600', delay: 350 },
          { text: '  Configuring nginx & PostgreSQL...', cls: 'text-zinc-600', delay: 500 },
          { text: '  Starting services...', cls: 'text-zinc-600', delay: 400 },
          { text: '\u00A0', cls: '', delay: 250 },
          // No version number here. It said "v2.8.0" \u2014 thirty-six releases
          // stale \u2014 because a hardcoded version in a decorative animation is
          // something nobody ever remembers to bump. The elapsed time is the
          // point of the line anyway.
          //
          // The download size on the line above was left behind by that same
          // edit: the version came out for being un-bumpable and the "41 MB"
          // beside it, wrong by a factor of two, stayed. Both were decoration;
          // only one was noticed. It is quoted from the register now.
          { text: `\u2713 DockPanel installed in ${m.installSeconds}s`, cls: 'text-emerald-400 font-medium', delay: 350 },
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
    // No macOS traffic lights. This product manages Linux servers, and a fake
    // Mac window frame around a root shell is a costume \u2014 it was on the hero
    // terminal and on all five product screenshots. What replaces it is the
    // thing a real session actually shows: who you are and where you are.
    <div ref={ref} className="w-full max-w-3xl border border-[#1e1e22] bg-[#0d0d0f]">
      <div className="mono text-[11px] text-[#3f3f46] px-4 py-2.5 border-b border-[#1e1e22] flex items-center justify-between">
        <span>root@srv1:~</span>
        <span aria-hidden="true">{phase === 'done' ? 'exit 0' : 'ssh'}</span>
      </div>
      <div className="p-4 sm:p-5 font-mono text-[13px] leading-relaxed min-h-[220px]">
        {phase === 'typing' && (
          <div>
            <span className="text-[#3f3f46]">$ </span>
            <span className="text-[#e4e4e7]">{typing}</span>
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

  // The figure sits OUTSIDE the fill, in a fixed column, for the reason this
  // chart exists: DockPanel is a small fraction of an 800 MB scale, so its bar
  // is narrow and a label printed inside it was clipped to nothing — the one
  // number the chart is making an argument about was the only one you could
  // not read. Putting every figure in the same right-hand column also means
  // they align, which is what makes a set of measurements comparable at all.
  return (
    <div ref={ref} className="flex items-center gap-3 sm:gap-4">
      <span className={`w-20 sm:w-24 text-right text-sm shrink-0 ${highlight ? 'text-[#f4f4f5] font-medium' : 'text-[#6b6b74]'}`}>
        {name}
      </span>
      <div className="flex-1 h-8 bg-[#101012] overflow-hidden border border-[#1e1e22]">
        <motion.div
          initial={{ width: 0 }}
          animate={inView ? { width: `${width}%` } : { width: 0 }}
          transition={{ duration: 0.8, delay, ease: [0.23, 1, 0.32, 1] }}
          className={`h-full ${highlight ? 'bg-[#f4f4f5]' : 'bg-[#2a2a30]'}`}
        />
      </div>
      <span className={`mono tnum text-xs w-16 shrink-0 text-right ${highlight ? 'text-[#f4f4f5]' : 'text-[#6b6b74]'}`}>
        {mb} MB
      </span>
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
    title: `${m.templates} templates. One click.`,
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
  { name: `${m.templates} Docker Templates`, desc: 'One-click deploy across 14 categories', icon: Box },
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
  { name: 'Reseller & White-Label', desc: 'Custom branding, sub-accounts, billing integration', icon: Users },
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
  { q: 'System requirements?', a: '512 MB RAM, 1 CPU, 10 GB disk. Runs on Ubuntu, Debian, CentOS, Rocky, AlmaLinux, and Fedora. ARM64 works too.' },
  { q: 'How is this different from cPanel?', a: `cPanel uses ~${m.competitorRam.cPanel} MB of RAM, costs $15/month, and doesn't support Docker. DockPanel's two services take about ${m.ram.services} MB between them, or roughly ${m.ram.fullStack} MB counting the bundled PostgreSQL, cost nothing, and ship with ${m.templates} Docker templates, a WAF, passkey authentication, Git deploys, a CLI, and multi-server management.` },
  { q: 'What happens if DockPanel goes down?', a: 'Your sites keep running. Nginx and Docker are independent processes \u2014 the panel is just the management layer. It auto-restarts via systemd if it ever stops.' },
  { q: 'Can I manage multiple servers?', a: 'As many as you want. Install a lightweight agent on each server and manage them all from one dashboard.' },
  // "On a $5 VPS, that's the difference between running 20 sites and running 2"
  // used to close this answer. Nothing derives 20 or 2, and a page arguing from
  // measurement cannot afford a figure it invented. The measured ones make the
  // point without it.
  { q: 'Why Rust?', a: `${m.binary.total} MB of binaries on disk and about ${m.ram.services} MB of RAM for the two panel services at idle — no JVM, no Node, no Python runtime to keep patched. The panel is a process you can forget about rather than a stack you maintain.` },
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
    <div className="min-h-screen bg-[#0a0a0b] text-[#a3a3ad] selection:bg-white/15 selection:text-white overflow-x-hidden">

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
      <nav className={`fixed top-0 left-0 right-0 z-50 transition-all duration-300 ${scrolled ? 'bg-[#0a0a0b]/80 backdrop-blur-2xl border-b border-zinc-800/60' : 'border-b border-transparent'}`}>
        <div className="max-w-7xl mx-auto px-6 h-14 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2.5">
            <Logo className="h-6 w-6" />
            <span className="text-[17px] font-semibold text-[#f4f4f5] tracking-[-0.01em]">DockPanel</span>
          </Link>
          <div className="hidden md:flex items-center gap-8 text-[13px] font-medium text-zinc-500">
            <NavLink href="#features" label="Features" active={activeSection === 'features'} />
            <NavLink href="#compare" label="Compare" active={activeSection === 'compare'} />
            <Link to="/security" className="relative hover:text-white transition-colors pb-1">Security</Link>
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
                <Link to="/security" onClick={() => setMobileMenu(false)} className="text-zinc-300 hover:text-white transition-colors">Security</Link>
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

      {/* ── Hero ──────────────────────────────────────────────────────────
          Rebuilt s271. It was a centred column: two pill badges, an 88px
          headline, a centred subhead, a centred command, two centred
          buttons. Every landing page in this category looks like that.

          The thesis here is the SPEC SHEET, because the product's entire
          argument is a set of measurements — and a spec sheet is left-aligned
          by nature. The headline states the claim, the figures beside it are
          the evidence, and the install command is the largest interactive
          thing on the screen because copying it is the only thing a visitor
          is here to do.                                                    */}
      <section className="relative pt-24 sm:pt-32 lg:pt-40 pb-4">
        <div className="max-w-7xl mx-auto px-6">
          <motion.div
            initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.5, ease: [0.16, 1, 0.3, 1] }}
            className="grid lg:grid-cols-12 gap-y-12 lg:gap-x-16 items-start"
          >
            <div className="lg:col-span-7">
              {/* Was two pill badges. A licence and an implementation language
                  are facts, so they are set as facts, in the margin. */}
              <p className="eyebrow mb-7">Open source &nbsp;·&nbsp; Written in Rust</p>

              {/* Every clause here is checkable against the table beside it.
                  A first draft read "…one binary you can fit on a floppy",
                  which is a nice sentence about a 22 MB binary and a 1.44 MB
                  disk, i.e. false by a factor of fifteen. Specific beats
                  clever, and on a page whose entire argument is measurement,
                  one decorative exaggeration discredits the real figures. */}
              <h1 className="text-[2.5rem] sm:text-[3.25rem] lg:text-[3.75rem] font-semibold text-[#f4f4f5] leading-[1.06] tracking-[-0.03em]">
                Everything a server needs,
                <br />in one static binary
                <span className="text-[#6b6b74]">
                  <br />with nothing underneath.
                </span>
              </h1>

              <p className="mt-7 text-[17px] sm:text-lg leading-relaxed text-[#a3a3ad] max-w-xl">
                Sites, Docker apps, databases, email, monitoring, security, backups
                and Git deploys. One install, no runtime to keep alive, nothing to pay.
              </p>
            </div>

            {/* The evidence, as an instrument reads it. Mono, tabular, ruled —
                and labelled precisely enough that the memory figure cannot be
                mistaken for the download size, which is what the old headline
                did by putting the RAM figure next to the product name.

                THE ONE ANIMATION ON THIS PAGE, and it earns its place by
                reporting something: the rows arrive in order and each figure
                SETTLES on its value rather than being printed, which is what
                an instrument taking a reading looks like. A page whose whole
                argument is measurement should show the measurement happening.
                It runs once, on load, and nothing loops.

                Motion is off entirely for anyone who has asked for that — the
                CSS media query cannot stop a JS spring, so the counter checks
                the same preference itself and renders the final value. */}
            <dl className="lg:col-span-5 mono text-sm border-t border-[#1e1e22]">
              {[
                ['panel binary', m.binary.api, 'MB', `static; +${m.binary.agent} MB agent, +${m.binary.cli} MB CLI`],
                ['memory, both services', m.ram.services, 'MB', 'api + agent, cgroup accounting'],
                ['install to login', m.installSeconds, 's', 'Ubuntu 24.04, 1 vCPU'],
                ['runtime deps', 0, '', 'no Node, no Python, no PHP'],
              ].map(([label, n, unit, note], i) => (
                <motion.div
                  key={label as string}
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  transition={{ duration: 0.35, delay: 0.25 + i * 0.13 }}
                  className="flex items-baseline justify-between gap-4 py-3.5 border-b border-[#1e1e22]"
                >
                  <dt className="text-[#6b6b74] shrink-0">{label as string}</dt>
                  <dd className="flex items-baseline gap-3 min-w-0">
                    <span className="hidden sm:block text-[11px] text-[#3f3f46] truncate">{note as string}</span>
                    <span className="tnum text-[#f4f4f5] tabular-nums text-base">
                      <Counter value={n as number} delay={0.3 + i * 0.13} />
                      <span className="text-[#6b6b74] ml-0.5">{unit as string}</span>
                    </span>
                  </dd>
                </motion.div>
              ))}
            </dl>
          </motion.div>

          {/* The command. Full width of the text column, monospaced, and big
              enough to read across a room — it is the product's front door. */}
          <motion.div
            initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.5, delay: 0.12, ease: [0.16, 1, 0.3, 1] }}
            className="mt-14 max-w-3xl"
          >
            <div className="flex items-stretch border border-[#1e1e22] bg-[#101012] hover:border-[#2a2a30] transition-colors">
              <span className="mono select-none px-4 sm:px-5 flex items-center text-[#3f3f46] border-r border-[#1e1e22]">$</span>
              <code className="mono flex-1 min-w-0 overflow-x-auto whitespace-nowrap px-4 sm:px-5 py-4 text-[13px] sm:text-[15px] text-[#f4f4f5]">
                curl -sL dockpanel.dev/install.sh | sudo bash
              </code>
              <button
                onClick={handleCopy}
                aria-label="Copy the install command"
                className="mono shrink-0 px-4 sm:px-5 text-xs text-[#a3a3ad] hover:text-[#f4f4f5] hover:bg-[#16161a] border-l border-[#1e1e22] transition-colors flex items-center gap-2"
              >
                {copied ? <Check className="w-3.5 h-3.5 text-emerald-400" /> : <Copy className="w-3.5 h-3.5" />}
                {copied ? 'copied' : 'copy'}
              </button>
            </div>

            <div className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-3">
              <a href="https://docs.dockpanel.dev" className="text-sm font-medium text-[#f4f4f5] underline decoration-[#3f3f46] hover:decoration-[#f4f4f5] underline-offset-4 transition-colors">
                Read the docs
              </a>
              <a href="https://github.com/ovexro/dockpanel" className="text-sm font-medium text-[#a3a3ad] hover:text-[#f4f4f5] transition-colors inline-flex items-center gap-1.5">
                <Github className="w-3.5 h-3.5" /> Source
              </a>
              <p className="mono text-[11px] text-[#3f3f46] ml-auto">
                ubuntu · debian · rocky · alma · centos · fedora · arm64
              </p>
            </div>
          </motion.div>

          {/* The dashboard, shown at a size where it can actually be read.
              The macOS traffic-light chrome is gone: this product runs on
              Linux servers, and a fake Mac window frame is a costume. */}
          <motion.figure
            initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.6, delay: 0.24, ease: [0.16, 1, 0.3, 1] }}
            className="mt-20 lg:mt-24"
          >
            <img
              src="/screenshots/dp-dashboard.png"
              alt="The DockPanel dashboard: CPU, memory and disk over 24 hours, with uptime, sites, databases and container counts beneath."
              className="w-full block border border-[#1e1e22]"
            />
            {/* Names no host. The demo box is private, and a landing page is
                the wrong place to point traffic at it. */}
            <figcaption className="mono text-[11px] text-[#3f3f46] mt-3">
              the dashboard, unmodified — a real box, real numbers
            </figcaption>
          </motion.figure>
        </div>
      </section>

      {/* ── Install ───────────────────────────────────────────────────────
          Was two sections: a strip of six evenly-spaced counters, then
          "How it works." as three centred columns of icon chips under
          STEP 1 / STEP 2 / STEP 3.

          The counter strip went because the hero's spec table already states
          those figures, better labelled — and repeating a number is how the
          two copies start disagreeing.

          The steps went because an icon in a rounded square is a picture of
          nothing. Installing IS a terminal session, so the terminal plays it:
          the real command, the real output, the real elapsed time. The three
          steps survive as what they actually are — a caption naming what
          happens after the command returns.                                */}
      <section className="rule py-20 lg:py-28">
        <div className="max-w-7xl mx-auto px-6">
          <div className="measure">
            <p className="eyebrow lg:pt-2">Install</p>

            <div className="min-w-0">
              <h2 className="text-[1.75rem] sm:text-[2.25rem] font-semibold text-[#f4f4f5] leading-[1.15] tracking-[-0.02em] max-w-2xl">
                One command, and the box is serving.
              </h2>
              <p className="mt-4 text-[#a3a3ad] max-w-xl leading-relaxed">
                No package repository to add, no runtime to install first, and nothing
                to configure before it will start.
              </p>

              <div className="mt-10">
                <AnimatedTerminal />
              </div>

              <dl className="mt-10 grid sm:grid-cols-3 gap-x-10 gap-y-6 max-w-3xl">
                {steps.map((step) => (
                  <div key={step.title}>
                    <dt className="mono text-[13px] text-[#f4f4f5] mb-1.5">{step.title}</dt>
                    <dd className="text-[14px] text-[#6b6b74] leading-relaxed">{step.desc}</dd>
                  </div>
                ))}
              </dl>
            </div>
          </div>
        </div>
      </section>

      {/* ── Feature showcase ──────────────────────────────────────────────
          The strongest material on the page was already here: real
          screenshots of a real product, with copy that has a voice. Two
          things were wasting it — a fake Mac window frame on every shot, and
          a two-up grid that rendered each one ~330px wide, at which size the
          product UI is grey texture rather than evidence.

          One per row now, full measure, alternating which side the text sits
          on so the eye has somewhere to go. Nothing was added; the existing
          content was simply given the room to be legible.                  */}
      <section id="features" className="rule py-24 lg:py-32">
        <div className="max-w-7xl mx-auto px-6">
          <div className="measure mb-16 lg:mb-20">
            <p className="eyebrow lg:pt-2">What you get</p>
            <h2 className="text-[1.75rem] sm:text-[2.25rem] font-semibold text-[#f4f4f5] leading-[1.15] tracking-[-0.02em] max-w-2xl">
              Four things most panels sell separately, or not at all.
            </h2>
          </div>

          <div className="space-y-20 lg:space-y-28">
            {showcase.map((feat, i) => (
              <motion.div key={i}
                initial={{ opacity: 0, y: 16 }}
                whileInView={{ opacity: 1, y: 0 }}
                viewport={{ once: true, margin: '-60px' }}
                transition={{ duration: 0.5, ease: [0.16, 1, 0.3, 1] }}
                className="grid lg:grid-cols-12 gap-y-6 lg:gap-x-12 items-start"
              >
                <div className={`lg:col-span-4 ${i % 2 === 1 ? 'lg:order-2' : ''}`}>
                  <p className="mono text-[11px] text-[#3f3f46] mb-3 tnum">
                    {String(i + 1).padStart(2, '0')} / {String(showcase.length).padStart(2, '0')}
                  </p>
                  <h3 className="text-xl sm:text-2xl font-semibold text-[#f4f4f5] mb-3 leading-snug tracking-[-0.015em]">
                    {feat.title}
                  </h3>
                  <p className="text-[15px] text-[#a3a3ad] leading-relaxed">{feat.desc}</p>
                </div>

                <button
                  type="button"
                  onClick={() => setLightbox({ src: feat.shot, alt: feat.alt })}
                  aria-label={`Enlarge: ${feat.alt}`}
                  className={`lg:col-span-8 block w-full text-left border border-[#1e1e22] hover:border-[#2a2a30] transition-colors ${i % 2 === 1 ? 'lg:order-1' : ''}`}
                >
                  <img src={feat.shot} alt={feat.alt} className="w-full block" loading="lazy" />
                </button>
              </motion.div>
            ))}
          </div>
        </div>
      </section>

      {/* ── The full list ─────────────────────────────────────────────────
          Was 26 bordered cards, each with a rounded-square lucide icon. The
          icons were chosen loosely — a lightning bolt for webhooks, a palette
          for themes, a globe for a CDN — so they carried no information and
          took the eye first anyway.

          What a reader wants from a list of 26 modules is to scan it, so it
          is a list: ruled rows, name and description on one line, nothing
          competing. The count is stated because 26 is itself the argument. */}
      <section className="rule py-20 lg:py-28">
        <div className="max-w-7xl mx-auto px-6">
          <div className="measure">
            <p className="eyebrow lg:pt-2 tnum">{allFeatures.length} modules</p>

            <div className="min-w-0">
              <h2 className="text-[1.75rem] sm:text-[2.25rem] font-semibold text-[#f4f4f5] leading-[1.15] tracking-[-0.02em] max-w-2xl">
                All of it ships in the box.
              </h2>
              <p className="mt-4 text-[#a3a3ad] max-w-xl leading-relaxed">
                Nothing here is an add-on, a paid tier, or a plugin you have to find.
              </p>

              <ul className="mt-10 border-t border-[#1e1e22] grid md:grid-cols-2 md:gap-x-12">
                {allFeatures.map(({ name, desc }) => (
                  <li key={name} className="border-b border-[#1e1e22] py-3.5 flex items-baseline gap-4">
                    <span className="mono text-[13px] text-[#f4f4f5] shrink-0 w-[8.5rem] sm:w-[10rem]">{name}</span>
                    <span className="text-[13px] text-[#6b6b74] leading-relaxed min-w-0">{desc}</span>
                  </li>
                ))}
              </ul>
            </div>
          </div>
        </div>
      </section>

      {/* ── Comparison ────────────────────────────────────────────────────
          The RAM bars were already the best idea on the page — a real
          measurement, drawn to scale, where the product's whole claim is
          legible in one glance. They keep their prominence; only the framing
          changed, from a centred "The numbers." to a heading that says what
          the reader is looking at.                                         */}
      <section id="compare" className="rule py-24 lg:py-32">
        <div className="max-w-7xl mx-auto px-6">
          <div className="measure mb-14">
            <p className="eyebrow lg:pt-2">Compared</p>
            <div className="min-w-0">
              <h2 className="text-[1.75rem] sm:text-[2.25rem] font-semibold text-[#f4f4f5] leading-[1.15] tracking-[-0.02em] max-w-2xl">
                Memory at idle, drawn to scale.
              </h2>
              <p className="mt-4 text-[#a3a3ad] max-w-xl leading-relaxed">
                Every megabyte the panel holds is a megabyte your applications do not get.
                Each figure below is the whole stack, database included.
              </p>
            </div>
          </div>

          <div className="max-w-4xl space-y-3 mb-16">
            <RamBar name="DockPanel" mb={m.ram.fullStack} max={m.competitorRam.cPanel} highlight delay={0} />
            <RamBar name="CloudPanel" mb={m.competitorRam.cloudPanel} max={m.competitorRam.cPanel} delay={0.1} />
            <RamBar name="Plesk" mb={m.competitorRam.plesk} max={m.competitorRam.cPanel} delay={0.2} />
            <RamBar name="HestiaCP" mb={m.competitorRam.hestiaCp} max={m.competitorRam.cPanel} delay={0.3} />
            <RamBar name="cPanel" mb={m.competitorRam.cPanel} max={m.competitorRam.cPanel} delay={0.4} />
          </div>

          <motion.div initial={{ opacity: 0, y: 15 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true }}
            className="max-w-4xl overflow-x-auto border border-[#1e1e22]"
          >
            <table className="w-full text-left min-w-[580px]">
              <thead>
                <tr className="border-b border-[#1e1e22]">
                  {['', 'Install', 'Price', 'Docker', 'Self-hosted'].map(h => (
                    <th key={h} className={`px-5 py-4 mono text-[11px] font-normal text-[#3f3f46] uppercase tracking-[0.14em] ${h === 'Docker' || h === 'Self-hosted' ? 'text-center' : ''}`}>{h}</th>
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

      {/* ── Price ─────────────────────────────────────────────────────────
          A 9xl "$0" centred on the page was the one place the design raised
          its voice, and it raised it about a number that is zero. The
          interesting claim is not the digit, it is the list of things that
          are not metered — so the terms are set as terms, and the figure
          sits in the margin column like every other measurement.          */}
      <section id="pricing" className="rule py-24 lg:py-32">
        <div className="max-w-7xl mx-auto px-6">
          <div className="measure">
            <p className="eyebrow lg:pt-2">Price</p>

            <div className="min-w-0">
              <h2 className="text-[1.75rem] sm:text-[2.25rem] font-semibold text-[#f4f4f5] leading-[1.15] tracking-[-0.02em] max-w-2xl">
                Free, and specific about it.
              </h2>

              <dl className="mt-9 max-w-2xl border-t border-[#1e1e22] mono text-[13px]">
                {[
                  ['per month', '$0'],
                  ['per server', '$0'],
                  ['per site', '$0'],
                  ['feature tiers', 'none'],
                  ['usage limits', 'none'],
                  ['licence', 'BSL 1.1 → MIT in 2030'],
                ].map(([k, v]) => (
                  <div key={k} className="flex items-baseline justify-between gap-6 py-3 border-b border-[#1e1e22]">
                    <dt className="text-[#6b6b74]">{k}</dt>
                    <dd className="text-[#f4f4f5] tnum">{v}</dd>
                  </div>
                ))}
              </dl>

              <div className="mt-9 max-w-2xl flex flex-wrap items-center gap-x-6 gap-y-3">
                <a href="https://github.com/ovexro/dockpanel" className="inline-flex items-center gap-2 text-sm font-medium text-[#f4f4f5] underline decoration-[#3f3f46] hover:decoration-[#f4f4f5] underline-offset-4 transition-colors">
                  <Github className="w-3.5 h-3.5" /> View on GitHub
                </a>
                <a href="https://docs.dockpanel.dev" className="text-sm font-medium text-[#a3a3ad] hover:text-[#f4f4f5] transition-colors">
                  Read the docs
                </a>
                <p className="mono text-[11px] text-[#3f3f46] ml-auto">
                  stuck? <a href="mailto:hello@dockpanel.dev" className="text-[#6b6b74] hover:text-[#f4f4f5] underline underline-offset-4 decoration-[#3f3f46] transition-colors">hello@dockpanel.dev</a>
                </p>
              </div>
            </div>
          </div>
        </div>
      </section>

      {/* ── FAQ ──────────────────────────────────────────────────── */}
      <section id="faq" className="rule py-20 lg:py-28">
        <div className="max-w-7xl mx-auto px-6 measure">
          <p className="eyebrow lg:pt-2">Questions</p>
          <div className="min-w-0 max-w-3xl border-t border-[#1e1e22]">
            {faqs.map((faq, i) => (
              <div key={i} className="border-b border-[#1e1e22]">
                <button onClick={() => setOpenFaq(openFaq === i ? null : i)}
                  aria-expanded={openFaq === i}
                  className="w-full flex items-center justify-between gap-4 py-5 text-left group"
                >
                  <span className="text-[15px] font-medium text-[#e4e4e7] group-hover:text-white transition-colors">{faq.q}</span>
                  <ChevronDown className={`w-4 h-4 text-[#3f3f46] group-hover:text-[#a3a3ad] transition-all duration-200 shrink-0 ${openFaq === i ? 'rotate-180' : ''}`} />
                </button>
                <AnimatePresence>
                  {openFaq === i && (
                    <motion.div initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={{ duration: 0.15 }} className="overflow-hidden">
                      <p className="pb-5 text-[14px] text-[#6b6b74] leading-relaxed max-w-2xl">{faq.a}</p>
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
                <Logo className="h-6 w-6" />
                <span className="text-[17px] font-semibold text-[#f4f4f5] tracking-[-0.01em]">DockPanel</span>
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
                  <Link to="/security" className="hover:text-zinc-300 transition-colors">Security</Link>
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
