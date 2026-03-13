export default function Pricing() {
  return (
    <section id="pricing" className="py-24">
      <div className="mx-auto max-w-6xl px-6">
        <div className="text-center">
          <p className="text-sm font-medium uppercase tracking-wider text-brand-400">Pricing</p>
          <h2 className="mt-3 text-3xl font-bold tracking-tight text-white md:text-4xl">
            Free. Forever. No strings attached.
          </h2>
          <p className="mx-auto mt-4 max-w-2xl text-surface-200/60">
            Every feature is included. No artificial limits. No premium tiers.
            DockPanel is free and open source because server management shouldn't cost extra.
          </p>
        </div>

        <div className="mx-auto mt-16 max-w-2xl">
          {/* Main card */}
          <div className="relative rounded-2xl border border-brand-500/30 bg-brand-500/[0.06] p-8 md:p-10 pricing-glow">
            <div className="absolute -top-3 right-6 rounded-full bg-gradient-to-r from-brand-600 to-brand-500 px-3.5 py-1 text-xs font-semibold text-white shadow-lg shadow-brand-600/30">
              Open Source
            </div>

            <div className="flex items-center justify-between">
              <div>
                <h3 className="text-2xl font-bold text-white">DockPanel</h3>
                <p className="mt-1 text-sm text-surface-200/50">Self-hosted server management panel</p>
              </div>
              <div className="text-right">
                <span className="text-5xl font-extrabold text-white">$0</span>
                <p className="mt-1 text-sm text-surface-200/50">forever</p>
              </div>
            </div>

            <div className="mt-8 grid gap-3 sm:grid-cols-2">
              {[
                'Unlimited servers',
                'Unlimited sites',
                'Free SSL certificates',
                'Database management',
                '34 Docker app templates',
                'Scheduled backups & S3',
                'Web terminal & file manager',
                'Git deploy & staging',
                'DNS management',
                'Cron job management',
                'Uptime monitoring',
                'Alert rules & notifications',
                'Security scanning',
                '2FA / TOTP authentication',
                'Auto-healing engine',
                'Smart diagnostics',
                'Developer CLI',
                'Infrastructure as Code (YAML)',
                'Teams & multi-user',
                'Activity audit log',
                'ARM64 / homelab support',
              ].map((feature) => (
                <div key={feature} className="flex items-start gap-3 text-sm text-surface-200/60">
                  <svg className="mt-0.5 h-4 w-4 shrink-0 text-brand-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                  </svg>
                  {feature}
                </div>
              ))}
            </div>

            <div className="mt-8 flex flex-col gap-3 sm:flex-row">
              <a
                href="https://github.com/ovexro/dockpanel"
                className="inline-flex flex-1 items-center justify-center gap-2 rounded-xl bg-gradient-to-r from-brand-600 to-brand-500 py-3.5 text-sm font-semibold text-white shadow-lg shadow-brand-600/25 transition hover:shadow-brand-500/35"
              >
                <svg className="h-5 w-5" fill="currentColor" viewBox="0 0 24 24"><path d="M12 0C5.374 0 0 5.373 0 12c0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23A11.509 11.509 0 0112 5.803c1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576C20.566 21.797 24 17.3 24 12c0-6.627-5.373-12-12-12z"/></svg>
                View on GitHub
              </a>
              <a
                href="https://demo.dockpanel.dev"
                className="flex-1 rounded-xl border border-white/[0.08] bg-white/[0.03] py-3.5 text-center text-sm font-semibold text-surface-200/80 transition hover:bg-white/[0.06] hover:text-white"
              >
                Try Live Demo
              </a>
            </div>
          </div>

          {/* Support options */}
          <div className="mt-8 rounded-2xl border border-white/[0.06] bg-white/[0.015] p-6 md:p-8">
            <h4 className="text-lg font-semibold text-white">Need professional support?</h4>
            <p className="mt-2 text-sm text-surface-200/50">
              DockPanel is free to use, but if you want priority support, installation help,
              or custom feature development, we offer optional paid support plans.
            </p>
            <a
              href="mailto:hello@dockpanel.dev"
              className="mt-4 inline-block text-sm font-medium text-brand-400 transition hover:text-brand-300"
            >
              Contact us for support plans &rarr;
            </a>
          </div>
        </div>
      </div>
    </section>
  );
}
