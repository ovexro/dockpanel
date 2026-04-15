import { Link } from 'react-router-dom';
import {
  Server, ShieldCheck, FileCheck2, Clock, Mail, Github,
  ScanSearch, BadgeCheck, Lock, Package, AlertTriangle, ExternalLink,
} from 'lucide-react';

const hd = "font-['Space_Grotesk',system-ui,sans-serif]";

const POSTURE = {
  audits: 7,
  findingsClosed: 280,
  breaches: 0,
  latestAuditRound: 7,
  latestAuditDate: 'April 2026',
  currentVersion: 'v2.7.11',
};

const SLA = [
  { window: '48 hours', text: 'Acknowledge receipt of your report' },
  { window: '7 days', text: 'Initial assessment + severity classification' },
  { window: '90 days', text: 'Coordinated disclosure window before public write-up' },
];

const AUDIT_HISTORY = [
  {
    round: 7,
    date: 'April 2026',
    title: 'Regression + fresh CVE sweep',
    bullets: [
      'tar symlink following closed across three backup paths (--no-dereference)',
      'web-terminal denylist extended (chroot, pivot_root, capsh, mknod, debugfs, kexec)',
      'agent systemd unit hardened (ProtectKernelTunables, RestrictNamespaces=~CLONE_NEWUSER, +6 more)',
      'frontend URL guards reject javascript:/data: schemes on backend-controlled fields',
      'security-scan alert pileup eliminated (auto-resolves prior alerts before firing new ones)',
    ],
  },
  {
    round: 6,
    date: 'March 2026',
    title: 'Fresh zero-assumptions audit',
    bullets: [
      '30 findings closed across 24 files; six parallel agents, 222 Rust + 506 TS files',
      'MySQL SQL injection, deploy script RCE, CSRF X-Requested-With enforcement',
      'Compose YAML rewritten from string-matching to serde_yaml_ng AST parsing',
      'KDF upgrade (SHA-256 → HKDF, backwards-compatible legacy fallback)',
      'agent TLS strict-by-default, Stripe timing-attack hardening, mail injection, SMTP CRLF',
    ],
  },
  {
    round: 5,
    date: 'March 2026',
    title: 'Error-handling hardening',
    bullets: [
      '59 silent .ok() failures in the agent replaced with proper error handling + logging',
      '51 .ok().flatten() anti-patterns in the backend replaced with error propagation',
      '45+ command timeouts added (Docker, systemctl, apt) to prevent hanging',
    ],
  },
  {
    round: 4,
    date: 'March 2026',
    title: 'Feature gap audit',
    bullets: [
      'uninstall routes added for all 10 services (PHP, Certbot, UFW, Fail2Ban, PowerDNS, Redis, Node.js, Composer, mail, PHP versions)',
      'SSL certificate force-renewal + deletion endpoints',
      'user lifecycle: suspend/unsuspend with session invalidation, admin password reset',
      'installer hardening: silent package failures warn, Docker volume cleanup prevents DB password mismatch',
    ],
  },
  {
    round: 3,
    date: 'March 2026',
    title: 'Research-driven audit',
    bullets: [
      '55 findings (12 HIGH, 28 MEDIUM, 15 LOW) using real-world cPanel/HestiaCP/CyberPanel/CloudPanel/VestaCP/Webmin CVE patterns',
      'safe_command() with env_clear() applied to all 341 Command::new() calls (LD_PRELOAD/PATH hijack defense)',
      'all stored credentials (DB, SMTP, S3/SFTP, OAuth, TOTP, DKIM) encrypted at rest with AES-256-GCM',
      'database_backup.rs rewritten to pipe docker exec + gzip instead of bash -c with interpolated strings',
      'CSP headers, deploy log IDOR fixed, Docker exec denylist (7 escape commands)',
    ],
  },
  {
    round: 1,
    date: 'March 2026',
    title: 'Initial comprehensive audit (Rounds 1\u20132)',
    bullets: [
      '117 vulnerabilities resolved across 45 files',
      'command injection, path traversal, configuration injection',
      'missing authn/authz, privilege escalation, input validation gaps',
    ],
  },
];

const SUPPLY_CHAIN = [
  {
    icon: BadgeCheck,
    title: 'Signed releases (Sigstore + cosign)',
    since: 'v2.7.10+',
    text:
      'Every binary and SBOM in every GitHub release is signed in CI with cosign keyless. ' +
      'No long-lived signing key exists — the certificate is bound to the release workflow\u2019s ' +
      'GitHub Actions OIDC identity and recorded in the public Rekor transparency log.',
  },
  {
    icon: Package,
    title: 'Per-binary SBOMs',
    since: 'v2.7.10+',
    text:
      'cargo-sbom emits SPDX 2.3 documents for the agent, API, and CLI crates alongside ' +
      'every release. Each SBOM is also signed and verifiable with the same cosign command.',
  },
  {
    icon: ScanSearch,
    title: 'Per-image SBOM generation',
    since: 'v2.7.11+',
    text:
      'Operators can generate an SPDX 2.3 SBOM for any deployed Docker app on demand ' +
      '(syft). Useful for supply-chain audits and EU CRA compliance asks (mandatory September 2026).',
  },
  {
    icon: ShieldCheck,
    title: 'Per-image CVE scanning',
    since: 'v2.7.9+',
    text:
      'grype scans every running app\u2019s image and surfaces a severity badge per app row. ' +
      'A configurable soft deploy gate refuses new deploys above critical/high/medium thresholds.',
  },
];

const VERIFY_CMD = `cosign verify-blob \\
  --certificate dockpanel-agent-linux-amd64.pem \\
  --signature  dockpanel-agent-linux-amd64.sig \\
  --certificate-identity-regexp '^https://github\\.com/ovexro/dockpanel/\\.github/workflows/release\\.yml@refs/tags/v.+$' \\
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \\
  dockpanel-agent-linux-amd64`;

const RECENT_ADVISORIES = [
  { ver: '2.7.11', date: '2026-04-15', text: 'Per-image SBOM generation (syft) shipped — supply-chain transparency for every deployed container.' },
  { ver: '2.7.10', date: '2026-04-15', text: 'Signed releases via cosign keyless + per-binary SPDX SBOMs in CI.' },
  { ver: '2.7.9',  date: '2026-04-15', text: 'Per-image CVE scanning (grype) shipped; agent sandbox gap (/var/lib/dockpanel missing from ReadWritePaths) closed.' },
  { ver: '2.7.8',  date: '2026-04-15', text: 'Audit Round 7 fixes: tar --no-dereference, terminal denylist, agent unit hardening, URL guards, alert-pileup fix.' },
  { ver: '2.7.x',  date: '2026-03',    text: 'Audit Round 6: 30 findings closed including MySQL SQL injection, deploy RCE, CSRF, Compose YAML AST validation, HKDF.' },
  { ver: '2.7.x',  date: '2026-03',    text: 'Audit Round 3: research-driven sweep against real-world panel CVEs — 55 findings, env_clear() on 341 Command::new() calls, AES-256-GCM credentials at rest.' },
];

export default function Security() {
  return (
    <div className="min-h-screen bg-[#09090b] text-white">
      {/* Header */}
      <header className="border-b border-zinc-800 bg-zinc-950/80 backdrop-blur-xl">
        <div className="mx-auto flex max-w-5xl items-center justify-between px-6 py-4">
          <Link to="/" className="flex items-center gap-2.5">
            <div className="w-8 h-8 rounded-lg bg-emerald-500 flex items-center justify-center">
              <Server className="w-5 h-5 text-zinc-950" />
            </div>
            <span className={`text-lg font-bold tracking-tight text-white ${hd}`}>
              Dock<span className="text-emerald-400">Panel</span>
            </span>
          </Link>
          <Link to="/" className="text-sm text-zinc-400 transition hover:text-white">
            &larr; Back to home
          </Link>
        </div>
      </header>

      <main className="mx-auto max-w-5xl px-6 py-16">
        {/* Hero */}
        <section>
          <div className="inline-flex items-center gap-2 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-3 py-1 text-[11px] font-semibold uppercase tracking-widest text-emerald-300">
            <ShieldCheck className="h-3.5 w-3.5" /> Security posture
          </div>
          <h1 className={`mt-4 text-4xl font-bold tracking-tight text-white md:text-5xl ${hd}`}>
            Security is the product.
          </h1>
          <p className="mt-4 max-w-2xl text-zinc-400 leading-relaxed">
            Every other panel that skipped this page eventually wrote a postmortem instead.
            DockPanel takes the opposite approach: continuous internal audits, supply-chain
            transparency, signed releases, and a public response SLA. Here&apos;s the receipts.
          </p>
        </section>

        {/* Posture stats */}
        <section className="mt-10 grid grid-cols-2 gap-4 sm:grid-cols-4">
          {[
            { v: POSTURE.audits, l: 'Internal audit rounds' },
            { v: `${POSTURE.findingsClosed}+`, l: 'Findings closed' },
            { v: POSTURE.breaches, l: 'Reported breaches' },
            { v: POSTURE.currentVersion, l: 'Current release' },
          ].map((s, i) => (
            <div key={i} className="rounded-lg border border-zinc-800 bg-zinc-950/60 p-5">
              <div className={`text-3xl font-bold tabular-nums text-white ${hd}`}>{s.v}</div>
              <div className="mt-1 text-[12px] text-zinc-500">{s.l}</div>
            </div>
          ))}
        </section>
        <p className="mt-3 text-[12px] text-zinc-600">
          Latest audit: Round {POSTURE.latestAuditRound} ({POSTURE.latestAuditDate}). Counts cover all rounds combined.
        </p>

        {/* Supply chain */}
        <section className="mt-16">
          <h2 className={`text-2xl font-bold text-white ${hd}`}>Supply chain</h2>
          <p className="mt-2 text-sm text-zinc-500">
            What we ship, signed. What you run, scannable. EU CRA compliance lands September 2026 —
            DockPanel is ready today.
          </p>
          <div className="mt-6 grid gap-4 md:grid-cols-2">
            {SUPPLY_CHAIN.map((row) => (
              <div key={row.title} className="rounded-lg border border-zinc-800 bg-zinc-950/60 p-5">
                <div className="flex items-start justify-between gap-3">
                  <div className="flex items-center gap-2.5">
                    <row.icon className="h-5 w-5 text-emerald-400" />
                    <h3 className="text-[15px] font-semibold text-white">{row.title}</h3>
                  </div>
                  <span className="rounded border border-zinc-800 bg-zinc-900 px-1.5 py-0.5 text-[10px] font-mono text-zinc-400">
                    {row.since}
                  </span>
                </div>
                <p className="mt-3 text-[13px] leading-relaxed text-zinc-400">{row.text}</p>
              </div>
            ))}
          </div>

          <div className="mt-6 overflow-hidden rounded-lg border border-zinc-800 bg-zinc-950">
            <div className="flex items-center justify-between border-b border-zinc-800 bg-zinc-900/60 px-4 py-2">
              <span className="text-[11px] font-semibold uppercase tracking-widest text-zinc-500">
                Verify a downloaded binary
              </span>
              <a
                href="https://github.com/ovexro/dockpanel/blob/main/SECURITY.md#verifying-release-signatures"
                className="flex items-center gap-1 text-[11px] text-zinc-500 hover:text-white"
                target="_blank"
                rel="noopener noreferrer"
              >
                Docs <ExternalLink className="h-3 w-3" />
              </a>
            </div>
            <pre className="overflow-x-auto px-4 py-3 text-[12px] leading-relaxed text-emerald-200">
              <code>{VERIFY_CMD}</code>
            </pre>
          </div>
        </section>

        {/* SLA */}
        <section className="mt-16">
          <h2 className={`text-2xl font-bold text-white ${hd}`}>CVE response SLA</h2>
          <p className="mt-2 text-sm text-zinc-500">
            From <a href="https://github.com/ovexro/dockpanel/blob/main/SECURITY.md" className="text-emerald-400 underline underline-offset-2 hover:text-emerald-300" target="_blank" rel="noopener noreferrer">SECURITY.md</a>.
            We don&apos;t pursue legal action against good-faith researchers.
          </p>
          <div className="mt-6 grid gap-4 md:grid-cols-3">
            {SLA.map((row) => (
              <div key={row.window} className="rounded-lg border border-zinc-800 bg-zinc-950/60 p-5">
                <div className="flex items-center gap-2 text-emerald-400">
                  <Clock className="h-4 w-4" />
                  <span className="text-[11px] font-semibold uppercase tracking-widest">{row.window}</span>
                </div>
                <p className="mt-2 text-[13px] leading-relaxed text-zinc-300">{row.text}</p>
              </div>
            ))}
          </div>
        </section>

        {/* Audit history */}
        <section className="mt-16">
          <h2 className={`text-2xl font-bold text-white ${hd}`}>What we audit ourselves for</h2>
          <p className="mt-2 text-sm text-zinc-500">
            Seven rounds. Each linked back to fixes in the public CHANGELOG. Most-recent first.
          </p>
          <div className="mt-6 space-y-4">
            {AUDIT_HISTORY.map((r) => (
              <div key={r.round} className="rounded-lg border border-zinc-800 bg-zinc-950/60 p-5">
                <div className="flex items-center gap-3">
                  <span className="flex h-7 w-7 items-center justify-center rounded-full bg-emerald-500/15 text-[12px] font-bold text-emerald-300">
                    {r.round}
                  </span>
                  <h3 className="text-[15px] font-semibold text-white">{r.title}</h3>
                  <span className="ml-auto text-[12px] text-zinc-500">{r.date}</span>
                </div>
                <ul className="mt-3 space-y-1.5 pl-10 text-[13px] leading-relaxed text-zinc-400">
                  {r.bullets.map((b, i) => (
                    <li key={i} className="list-disc marker:text-zinc-700">{b}</li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
          <div className="mt-4 text-[12px] text-zinc-600">
            Full write-ups in{' '}
            <a
              href="https://github.com/ovexro/dockpanel/blob/main/SECURITY.md#past-security-work"
              className="text-emerald-400 underline underline-offset-2 hover:text-emerald-300"
              target="_blank"
              rel="noopener noreferrer"
            >
              SECURITY.md &rarr; Past Security Work
            </a>.
          </div>
        </section>

        {/* Recent advisories */}
        <section className="mt-16">
          <h2 className={`text-2xl font-bold text-white ${hd}`}>Recent advisories addressed</h2>
          <p className="mt-2 text-sm text-zinc-500">
            What we&apos;ve shipped to harden the panel. From the{' '}
            <a
              href="https://github.com/ovexro/dockpanel/blob/main/CHANGELOG.md"
              className="text-emerald-400 underline underline-offset-2 hover:text-emerald-300"
              target="_blank"
              rel="noopener noreferrer"
            >
              CHANGELOG
            </a>.
          </p>
          <div className="mt-6 overflow-hidden rounded-lg border border-zinc-800">
            {RECENT_ADVISORIES.map((a, i) => (
              <div
                key={i}
                className={`flex flex-col gap-1 px-5 py-3 text-[13px] sm:flex-row sm:items-center sm:gap-4 ${
                  i % 2 === 0 ? 'bg-zinc-950/60' : 'bg-zinc-950/30'
                }`}
              >
                <span className="w-20 font-mono text-[12px] text-emerald-400">{a.ver}</span>
                <span className="w-24 text-[12px] text-zinc-600">{a.date}</span>
                <span className="text-zinc-300">{a.text}</span>
              </div>
            ))}
          </div>
        </section>

        {/* Architecture summary */}
        <section className="mt-16">
          <h2 className={`text-2xl font-bold text-white ${hd}`}>Defense in depth</h2>
          <p className="mt-2 text-sm text-zinc-500">
            The properties every DockPanel install ships with by default.
          </p>
          <div className="mt-6 grid gap-3 md:grid-cols-2">
            {[
              ['Unix socket agent', 'Agent never exposed to the network.'],
              ['Argon2 password hashing', 'Memory-hard, GPU-resistant.'],
              ['AES-256-GCM credentials at rest', 'DB, SMTP, S3/SFTP, OAuth, TOTP, DKIM.'],
              ['JWT auth + IDOR ownership checks', 'On every resource handler.'],
              ['env_clear() on Command::new()', 'No LD_PRELOAD / PATH hijacking.'],
              ['Rate-limited auth endpoints', 'Brute-force resistance.'],
              ['CSP on frontend nginx', 'XSS + injection mitigation.'],
              ['Systemd-hardened agent unit', 'ProtectSystem=strict + 10 protect/restrict directives.'],
              ['Terminal sandbox', 'PR_SET_NO_NEW_PRIVS, restricted bash, command blocklist.'],
              ['No telemetry, ever', 'Self-hosted means self-hosted.'],
            ].map(([t, d]) => (
              <div key={t} className="flex items-start gap-3 rounded-lg border border-zinc-800/60 bg-zinc-950/40 p-4">
                <Lock className="mt-0.5 h-4 w-4 flex-none text-emerald-400" />
                <div>
                  <div className="text-[13px] font-semibold text-white">{t}</div>
                  <div className="mt-0.5 text-[12px] text-zinc-500">{d}</div>
                </div>
              </div>
            ))}
          </div>
        </section>

        {/* Report a vulnerability */}
        <section className="mt-16 rounded-xl border border-emerald-500/30 bg-emerald-500/[0.04] p-8">
          <div className="flex items-center gap-3">
            <AlertTriangle className="h-5 w-5 text-emerald-400" />
            <h2 className={`text-xl font-bold text-white ${hd}`}>Report a vulnerability</h2>
          </div>
          <p className="mt-3 text-[14px] leading-relaxed text-zinc-300">
            Please don&apos;t open a public GitHub issue for security vulnerabilities. Email us
            with reproduction steps, impact, and any PoC. We acknowledge within 48 hours and
            credit researchers (with permission) who follow responsible disclosure.
          </p>
          <div className="mt-5 flex flex-wrap gap-3">
            <a
              href="mailto:security@dockpanel.dev"
              className="inline-flex items-center gap-2 rounded-md bg-white px-4 py-2 text-[13px] font-semibold text-zinc-900 transition hover:bg-zinc-200"
            >
              <Mail className="h-4 w-4" /> security@dockpanel.dev
            </a>
            <a
              href="https://github.com/ovexro/dockpanel/security/advisories/new"
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-2 rounded-md border border-zinc-800 bg-zinc-900/50 px-4 py-2 text-[13px] font-semibold text-zinc-200 transition hover:border-zinc-700"
            >
              <Github className="h-4 w-4" /> GitHub Security Advisory
            </a>
            <a
              href="https://github.com/ovexro/dockpanel/blob/main/SECURITY.md"
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-2 rounded-md border border-zinc-800 bg-zinc-900/50 px-4 py-2 text-[13px] font-semibold text-zinc-200 transition hover:border-zinc-700"
            >
              <FileCheck2 className="h-4 w-4" /> Full SECURITY.md
            </a>
          </div>
        </section>

        <div className="mt-16 border-t border-zinc-800/60 pt-6 text-[12px] text-zinc-600">
          Page reflects state at {POSTURE.currentVersion}. Audit and finding counts are tracked in{' '}
          <a
            href="https://github.com/ovexro/dockpanel/blob/main/SECURITY.md"
            className="text-emerald-400 underline underline-offset-2 hover:text-emerald-300"
            target="_blank"
            rel="noopener noreferrer"
          >
            SECURITY.md
          </a>{' '}
          and the{' '}
          <a
            href="https://github.com/ovexro/dockpanel/blob/main/CHANGELOG.md"
            className="text-emerald-400 underline underline-offset-2 hover:text-emerald-300"
            target="_blank"
            rel="noopener noreferrer"
          >
            CHANGELOG
          </a>.
        </div>
      </main>
    </div>
  );
}
