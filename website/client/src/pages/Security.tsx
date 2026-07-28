import { useEffect, useState } from 'react';
import { Github, Mail } from 'lucide-react';
import { DocSection, DocShell, DocTitle, docLink } from '../components/DocChrome';

/**
 * The security page, on the site's spine.
 *
 * It was the last page on a dialect of its own. Rebuilt Jul 27 16:29 — twenty-
 * nine minutes before the `.measure`/`.rule`/`.eyebrow` spine landed in
 * `Landing.tsx` at 16:58 — it used none of it: ground #09090b against the
 * token's #0a0a0b, a pill for an eyebrow, a local font constant restating what
 * index.css already sets on `body`, and no footer, so the page was a dead end.
 * s277 cleared it by mtime, which answers when it was touched rather than what
 * it is built on.
 *
 * Nothing it claims changed. Every figure, date, bullet and heading below is
 * the string that was already published — restyling is not amending, and a
 * security page is the last place to quietly restate a claim. What changed is
 * the drawing: the bordered cards with a lucide glyph apiece are ruled rows,
 * for the reason `Landing.tsx` gives where it retired the same pattern — an
 * icon chosen loosely carries no information and takes the eye first anyway.
 * Measurements are mono and tabular because they are measurements.
 */

const POSTURE = {
  audits: 7,
  findingsClosed: 280,
  breaches: 0,
  latestAuditRound: 7,
  latestAuditDate: 'April 2026',
  // NOT hardcoded. It said v2.7.11 — thirty-seven releases stale — because a
  // version typed into a page is a fact with no owner. It is fetched from the
  // releases API at render time, and the row is simply omitted if that fails:
  // a number nobody can verify is worse than no number.
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
    title: 'Initial comprehensive audit (Rounds 1–2)',
    bullets: [
      '117 vulnerabilities resolved across 45 files',
      'command injection, path traversal, configuration injection',
      'missing authn/authz, privilege escalation, input validation gaps',
    ],
  },
];

const SUPPLY_CHAIN = [
  {
    title: 'Signed releases (Sigstore + cosign)',
    since: 'v2.7.10+',
    text:
      'Every binary and SBOM in every GitHub release is signed in CI with cosign keyless. ' +
      'No long-lived signing key exists — the certificate is bound to the release workflow’s ' +
      'GitHub Actions OIDC identity and recorded in the public Rekor transparency log.',
  },
  {
    title: 'Per-binary SBOMs',
    since: 'v2.7.10+',
    text:
      'cargo-sbom emits SPDX 2.3 documents for the agent, API, and CLI crates alongside ' +
      'every release. Each SBOM is also signed and verifiable with the same cosign command.',
  },
  {
    title: 'Per-image SBOM generation',
    since: 'v2.7.11+',
    text:
      'Operators can generate an SPDX 2.3 SBOM for any deployed Docker app on demand ' +
      '(syft). Useful for supply-chain audits and EU CRA compliance asks (mandatory September 2026).',
  },
  {
    title: 'Per-image CVE scanning',
    since: 'v2.7.9+',
    text:
      'grype scans every running app’s image and surfaces a severity badge per app row. ' +
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

const DEFAULTS: [string, string][] = [
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
];

const SECURITY_MD = 'https://github.com/ovexro/dockpanel/blob/main/SECURITY.md';
const CHANGELOG_MD = 'https://github.com/ovexro/dockpanel/blob/main/CHANGELOG.md';

/** Section headings. Sized as sections rather than clauses — these are units. */
const h2 =
  'text-[1.5rem] sm:text-[1.75rem] font-semibold leading-[1.15] tracking-[-0.02em] text-[#f4f4f5]';
/** The line under a heading that says what the section is for. */
const standfirst = 'mt-3 text-[14px] leading-relaxed text-[#a3a3ad]';

export default function Security() {
  // See the note on POSTURE: the release number is read from GitHub rather
  // than typed here, so it cannot go stale. Failure leaves it null and the
  // row is dropped.
  const [latestRelease, setLatestRelease] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    fetch('https://api.github.com/repos/ovexro/dockpanel/releases/latest')
      .then((r) => (r.ok ? r.json() : null))
      .then((d) => { if (live && d?.tag_name) setLatestRelease(d.tag_name); })
      .catch(() => {});
    return () => { live = false; };
  }, []);

  const posture: [string, string][] = [
    ['internal audit rounds', String(POSTURE.audits)],
    ['findings closed', `${POSTURE.findingsClosed}+`],
    ['reported breaches', String(POSTURE.breaches)],
    ...(latestRelease ? ([['current release', latestRelease]] as [string, string][]) : []),
  ];

  return (
    <DocShell>
      <DocTitle
        eyebrow="Security posture"
        title="Security is the product."
        standfirst={
          <>
            Every other panel that skipped this page eventually wrote a postmortem instead.
            DockPanel takes the opposite approach: continuous internal audits, supply-chain
            transparency, signed releases, and a public response SLA. Here&apos;s the receipts.
          </>
        }
      />

      {/* ── Posture ───────────────────────────────────────────────────────
          Four bordered tiles with a 3xl figure apiece became four rows of a
          ruled list, which is the shape a set of measurements already has
          everywhere else on this site. Tabular figures so the digits line up,
          which is what makes them comparable at all.                       */}
      <DocSection id="posture" label="Posture" contentClassName="max-w-2xl">
        <dl className="mono border-t border-[#1e1e22] text-[13px]">
          {posture.map(([k, v]) => (
            <div
              key={k}
              className="flex items-baseline justify-between gap-6 border-b border-[#1e1e22] py-3"
            >
              <dt className="text-[#6b6b74]">{k}</dt>
              <dd className="tnum text-[#f4f4f5]">{v}</dd>
            </div>
          ))}
        </dl>
        <p className="mt-4 text-[12px] leading-relaxed text-[#6b6b74]">
          Latest audit: Round {POSTURE.latestAuditRound} ({POSTURE.latestAuditDate}). Counts
          cover all rounds combined.
        </p>
      </DocSection>

      {/* ── Provenance ────────────────────────────────────────────────── */}
      <DocSection id="supply-chain" label="Provenance" contentClassName="max-w-3xl">
        <h2 className={h2}>Supply chain</h2>
        <p className={standfirst}>
          What we ship, signed. What you run, scannable. EU CRA compliance lands September 2026 —
          DockPanel is ready today.
        </p>

        <ul className="mt-8 border-t border-[#1e1e22]">
          {SUPPLY_CHAIN.map((row) => (
            <li key={row.title} className="border-b border-[#1e1e22] py-4">
              <div className="flex items-baseline justify-between gap-4">
                <h3 className="text-[14px] font-semibold text-[#f4f4f5]">{row.title}</h3>
                <span className="mono shrink-0 text-[11px] text-[#3f3f46]">{row.since}</span>
              </div>
              <p className="mt-2 text-[13px] leading-relaxed text-[#6b6b74]">{row.text}</p>
            </li>
          ))}
        </ul>

        {/* The one thing on this page a reader can run. It is a command, so it
            is set as one — no rounded panel, just the rule and the mono. */}
        <figure className="mt-8 border border-[#1e1e22]">
          <figcaption className="flex items-center justify-between gap-4 border-b border-[#1e1e22] px-4 py-2.5">
            <span className="eyebrow">Verify a downloaded binary</span>
            <a
              href={`${SECURITY_MD}#verifying-release-signatures`}
              className="mono text-[11px] text-[#6b6b74] transition-colors hover:text-[#f4f4f5]"
              target="_blank"
              rel="noopener noreferrer"
            >
              Docs &#8599;
            </a>
          </figcaption>
          <pre className="overflow-x-auto px-4 py-3.5 text-[12px] leading-relaxed text-[#a3a3ad]">
            <code>{VERIFY_CMD}</code>
          </pre>
        </figure>
      </DocSection>

      {/* ── Response ──────────────────────────────────────────────────── */}
      <DocSection id="sla" label="Response" contentClassName="max-w-3xl">
        <h2 className={h2}>CVE response SLA</h2>
        <p className={standfirst}>
          From{' '}
          <a href={SECURITY_MD} className={docLink} target="_blank" rel="noopener noreferrer">
            SECURITY.md
          </a>
          . We don&apos;t pursue legal action against good-faith researchers.
        </p>

        <dl className="mt-8 border-t border-[#1e1e22]">
          {SLA.map((row) => (
            <div
              key={row.window}
              className="flex flex-col gap-1 border-b border-[#1e1e22] py-3.5 sm:flex-row sm:items-baseline sm:gap-6"
            >
              <dt className="mono tnum shrink-0 text-[13px] text-[#f4f4f5] sm:w-[7rem]">
                {row.window}
              </dt>
              <dd className="min-w-0 text-[13px] leading-relaxed text-[#6b6b74]">{row.text}</dd>
            </div>
          ))}
        </dl>
      </DocSection>

      {/* ── Audits ────────────────────────────────────────────────────── */}
      <DocSection id="audits" label="Audits" contentClassName="max-w-3xl">
        <h2 className={h2}>What we audit ourselves for</h2>
        <p className={standfirst}>
          Seven rounds. Each linked back to fixes in the public CHANGELOG. Most-recent first.
        </p>

        <div className="mt-8 border-t border-[#1e1e22]">
          {AUDIT_HISTORY.map((r) => (
            <article key={r.round} className="border-b border-[#1e1e22] py-5">
              <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1">
                <span className="mono tnum shrink-0 text-[13px] text-[#f4f4f5]">
                  Round {r.round}
                </span>
                <h3 className="min-w-0 text-[14px] font-semibold text-[#f4f4f5]">{r.title}</h3>
                <span className="mono ml-auto shrink-0 text-[11px] text-[#3f3f46]">{r.date}</span>
              </div>
              <ul className="mt-3 space-y-1.5 text-[13px] leading-relaxed text-[#6b6b74]">
                {r.bullets.map((b, i) => (
                  <li key={i} className="flex gap-3">
                    <span aria-hidden="true" className="mono select-none text-[#3f3f46]">
                      &mdash;
                    </span>
                    <span className="min-w-0">{b}</span>
                  </li>
                ))}
              </ul>
            </article>
          ))}
        </div>

        <p className="mt-5 text-[12px] text-[#6b6b74]">
          Full write-ups in{' '}
          <a
            href={`${SECURITY_MD}#past-security-work`}
            className={docLink}
            target="_blank"
            rel="noopener noreferrer"
          >
            SECURITY.md &rarr; Past Security Work
          </a>
          .
        </p>
      </DocSection>

      {/* ── Advisories ────────────────────────────────────────────────── */}
      <DocSection id="advisories" label="Advisories" contentClassName="max-w-3xl">
        <h2 className={h2}>Recent advisories addressed</h2>
        <p className={standfirst}>
          What we&apos;ve shipped to harden the panel. From the{' '}
          <a href={CHANGELOG_MD} className={docLink} target="_blank" rel="noopener noreferrer">
            CHANGELOG
          </a>
          .
        </p>

        <div className="mt-8 border-t border-[#1e1e22]">
          {RECENT_ADVISORIES.map((a, i) => (
            <div
              key={i}
              className="flex flex-col gap-1 border-b border-[#1e1e22] py-3.5 sm:flex-row sm:items-baseline sm:gap-6"
            >
              <span className="mono tnum shrink-0 text-[12px] text-[#f4f4f5] sm:w-[3.5rem]">
                {a.ver}
              </span>
              <span className="mono tnum shrink-0 text-[12px] text-[#3f3f46] sm:w-[5rem]">
                {a.date}
              </span>
              <span className="min-w-0 text-[13px] leading-relaxed text-[#6b6b74]">{a.text}</span>
            </div>
          ))}
        </div>
      </DocSection>

      {/* ── Defaults ──────────────────────────────────────────────────── */}
      <DocSection id="defaults" label="By default" contentClassName="max-w-3xl">
        <h2 className={h2}>Defense in depth</h2>
        <p className={standfirst}>The properties every DockPanel install ships with by default.</p>

        <dl className="mt-8 grid border-t border-[#1e1e22] md:grid-cols-2 md:gap-x-12">
          {DEFAULTS.map(([t, d]) => (
            <div key={t} className="border-b border-[#1e1e22] py-3.5">
              <dt className="mono text-[13px] text-[#f4f4f5]">{t}</dt>
              <dd className="mt-1 text-[13px] leading-relaxed text-[#6b6b74]">{d}</dd>
            </div>
          ))}
        </dl>
      </DocSection>

      {/* ── Disclosure ────────────────────────────────────────────────── */}
      <DocSection id="disclosure" label="Disclosure" contentClassName="max-w-2xl">
        <h2 className={h2}>Report a vulnerability</h2>
        <p className="mt-3 text-[15px] leading-relaxed text-[#a3a3ad]">
          Please don&apos;t open a public GitHub issue for security vulnerabilities. Email us
          with reproduction steps, impact, and any PoC. We acknowledge within 48 hours and
          credit researchers (with permission) who follow responsible disclosure.
        </p>
        <div className="mt-7 flex flex-wrap items-center gap-x-6 gap-y-3">
          <a
            href="mailto:security@dockpanel.dev"
            className="mono inline-flex items-center gap-2 text-[13px] text-[#f4f4f5] underline decoration-[#3f3f46] underline-offset-4 transition-colors hover:decoration-[#f4f4f5]"
          >
            <Mail className="h-3.5 w-3.5" /> security@dockpanel.dev
          </a>
          <a
            href="https://github.com/ovexro/dockpanel/security/advisories/new"
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-2 text-[13px] font-medium text-[#a3a3ad] transition-colors hover:text-[#f4f4f5]"
          >
            <Github className="h-3.5 w-3.5" /> GitHub Security Advisory
          </a>
          <a
            href={SECURITY_MD}
            target="_blank"
            rel="noopener noreferrer"
            className="text-[13px] font-medium text-[#a3a3ad] transition-colors hover:text-[#f4f4f5]"
          >
            Full SECURITY.md
          </a>
        </div>
      </DocSection>

      {/* ── Sources ───────────────────────────────────────────────────── */}
      <DocSection id="sources" label="Sources" contentClassName="max-w-2xl">
        <p className="text-[13px] leading-relaxed text-[#6b6b74]">
          Audit and finding counts are tracked in{' '}
          <a href={SECURITY_MD} className={docLink} target="_blank" rel="noopener noreferrer">
            SECURITY.md
          </a>{' '}
          and the{' '}
          <a href={CHANGELOG_MD} className={docLink} target="_blank" rel="noopener noreferrer">
            CHANGELOG
          </a>
          .
        </p>
      </DocSection>
    </DocShell>
  );
}
