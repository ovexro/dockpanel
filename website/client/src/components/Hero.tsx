import { useState } from 'react';

export default function Hero() {
  const [copied, setCopied] = useState(false);
  const installCmd = 'curl -sL https://dockpanel.dev/install.sh | sudo bash';

  const copyCommand = () => {
    navigator.clipboard.writeText(installCmd);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <section className="relative overflow-hidden pt-32 pb-20 md:pt-40 md:pb-28">
      {/* Background glow */}
      <div className="pointer-events-none absolute inset-0">
        <div className="absolute top-0 left-1/2 h-[600px] w-[800px] -translate-x-1/2 rounded-full bg-brand-600/8 blur-[120px]" />
        <div className="absolute top-40 right-0 h-[400px] w-[400px] rounded-full bg-brand-400/5 blur-[100px]" />
      </div>

      <div className="relative mx-auto max-w-6xl px-6">
        {/* Badge */}
        <div className="mb-8 flex justify-center">
          <div className="inline-flex items-center gap-2 rounded-full border border-brand-500/20 bg-brand-500/10 px-4 py-1.5 text-sm text-brand-300">
            <span className="h-1.5 w-1.5 rounded-full bg-brand-400 animate-pulse" />
            100% free &amp; open source — no subscriptions, no limits
          </div>
        </div>

        {/* Heading */}
        <h1 className="mx-auto max-w-4xl text-center text-4xl font-extrabold leading-[1.1] tracking-tight text-white md:text-6xl lg:text-7xl">
          Your server.{' '}
          <span className="bg-gradient-to-r from-brand-400 to-brand-300 bg-clip-text text-transparent">
            Your rules.
          </span>{' '}
          Your panel.
        </h1>

        <p className="mx-auto mt-6 max-w-2xl text-center text-lg leading-relaxed text-surface-200/60 md:text-xl">
          DockPanel is a free, self-hosted, Docker-native server management panel
          built in Rust. No vendor lock-in. No billing. No limits.
          Install in under 60 seconds and own your infrastructure.
        </p>

        {/* Install command */}
        <div className="mx-auto mt-10 max-w-xl">
          <div className="group relative overflow-hidden rounded-xl border border-white/10 bg-surface-900/80 backdrop-blur">
            <div className="flex items-center justify-between gap-4 px-5 py-4">
              <div className="flex items-center gap-3 min-w-0">
                <span className="shrink-0 text-sm text-brand-400">$</span>
                <code className="text-sm text-surface-200/80 break-all sm:whitespace-nowrap">{installCmd}</code>
              </div>
              <button
                onClick={copyCommand}
                aria-label="Copy install command to clipboard"
                className="shrink-0 rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs font-medium text-surface-200/70 transition hover:bg-white/10 hover:text-white"
              >
                {copied ? 'Copied!' : 'Copy'}
              </button>
            </div>
          </div>
          <p className="mt-3 text-center text-xs text-surface-200/60">
            Works on Ubuntu 20+, Debian 11+, CentOS 9+, Amazon Linux 2023 &mdash; x86_64 &amp; ARM64
          </p>
        </div>

        {/* CTA buttons */}
        <div className="mt-10 flex flex-col items-center justify-center gap-4 sm:flex-row">
          <a
            href="https://github.com/ovexro/dockpanel"
            className="inline-flex items-center gap-2 rounded-xl bg-brand-600 px-8 py-3.5 text-sm font-semibold text-white shadow-lg shadow-brand-600/25 transition hover:bg-brand-500 hover:shadow-brand-500/30"
          >
            <svg className="h-5 w-5" fill="currentColor" viewBox="0 0 24 24"><path d="M12 0C5.374 0 0 5.373 0 12c0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23A11.509 11.509 0 0112 5.803c1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576C20.566 21.797 24 17.3 24 12c0-6.627-5.373-12-12-12z"/></svg>
            View on GitHub
          </a>
          <a
            href="https://demo.dockpanel.dev"
            className="rounded-xl border border-white/10 bg-white/5 px-8 py-3.5 text-sm font-semibold text-surface-200/80 transition hover:bg-white/10 hover:text-white"
          >
            Live Demo
          </a>
        </div>

        {/* Social proof */}
        <div className="mt-16 flex flex-col items-center gap-6 md:flex-row md:justify-center md:gap-12">
          {[
            { value: '~10MB', label: 'Binary size' },
            { value: '12MB', label: 'RAM usage' },
            { value: '<60s', label: 'Install time' },
            { value: 'ARM64', label: 'Homelab ready' },
          ].map(({ value, label }) => (
            <div key={label} className="text-center">
              <div className="text-2xl font-bold text-white">{value}</div>
              <div className="mt-0.5 text-xs text-surface-200/60">{label}</div>
            </div>
          ))}
        </div>

        {/* Dashboard preview */}
        <div className="mx-auto mt-16 max-w-4xl">
          <div className="overflow-hidden rounded-2xl border border-white/[0.08] shadow-2xl shadow-black/50">
            {/* Browser chrome */}
            <div className="flex items-center gap-2 border-b border-white/5 bg-surface-900/90 px-4 py-3">
              <div className="h-3 w-3 rounded-full bg-red-500/60" />
              <div className="h-3 w-3 rounded-full bg-yellow-500/60" />
              <div className="h-3 w-3 rounded-full bg-green-500/60" />
              <div className="mx-auto max-w-xs flex-1">
                <div className="rounded-md bg-white/[0.04] px-3 py-1 text-center text-xs text-surface-200/30">
                  your-server:8080
                </div>
              </div>
              <div className="w-[52px]" />
            </div>

            <div className="flex min-h-[280px] bg-surface-900/60">
              {/* Sidebar */}
              <div className="hidden w-44 shrink-0 border-r border-white/5 bg-white/[0.015] p-3 md:block">
                <div className="mb-4 flex items-center gap-2 px-2">
                  <div className="flex h-5 w-5 items-center justify-center rounded bg-brand-600">
                    <svg viewBox="0 0 20 20" className="h-3 w-3 text-white" fill="currentColor">
                      <rect x="2" y="2" width="16" height="4" rx="1" opacity="0.9" />
                      <rect x="2" y="8" width="4" height="4" rx="0.5" opacity="0.7" />
                      <rect x="8" y="8" width="4" height="4" rx="0.5" opacity="0.7" />
                    </svg>
                  </div>
                  <span className="text-xs font-semibold text-white">DockPanel</span>
                </div>
                {[
                  { name: 'Dashboard', active: true },
                  { name: 'Sites', active: false },
                  { name: 'Databases', active: false },
                  { name: 'Docker Apps', active: false },
                  { name: 'Security', active: false },
                  { name: 'Terminal', active: false },
                  { name: 'Diagnostics', active: false },
                ].map(({ name, active }) => (
                  <div
                    key={name}
                    className={`mb-0.5 rounded-md px-2 py-1.5 text-[11px] ${
                      active
                        ? 'bg-brand-500/15 font-medium text-brand-300'
                        : 'text-surface-200/30'
                    }`}
                  >
                    {name}
                  </div>
                ))}
              </div>

              {/* Main content */}
              <div className="flex-1 p-4 md:p-5">
                <div className="mb-4 text-sm font-semibold text-white/90">Dashboard</div>
                <div className="grid grid-cols-3 gap-2.5">
                  {[
                    { label: 'Active Sites', value: '12', sub: 'All healthy' },
                    { label: 'SSL Certs', value: '12', sub: 'Auto-renew' },
                    { label: 'Databases', value: '8', sub: '2.1 GB used' },
                    { label: 'Docker Apps', value: '5', sub: 'All running' },
                    { label: 'Backups', value: '47', sub: 'Last: 2h ago' },
                    { label: 'CPU / RAM', value: '3%', sub: '12 MB used' },
                  ].map(({ label, value, sub }) => (
                    <div
                      key={label}
                      className="rounded-lg border border-white/[0.04] bg-white/[0.025] p-2.5 md:p-3"
                    >
                      <div className="text-[10px] uppercase tracking-wider text-surface-200/25">{label}</div>
                      <div className="mt-1 text-xl font-bold text-white">{value}</div>
                      <div className="mt-1 flex items-center gap-1 text-[10px] text-brand-300/70">
                        <span className="inline-block h-1.5 w-1.5 rounded-full bg-brand-400" />
                        {sub}
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
