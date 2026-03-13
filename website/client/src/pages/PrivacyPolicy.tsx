import { Link } from 'react-router-dom';

export default function PrivacyPolicy() {
  return (
    <div className="min-h-screen">
      {/* Simple header */}
      <header className="border-b border-white/5 bg-surface-950/80 backdrop-blur-xl">
        <div className="mx-auto flex max-w-4xl items-center justify-between px-6 py-4">
          <Link to="/" className="flex items-center gap-2.5">
            <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-brand-600">
              <svg viewBox="0 0 20 20" className="h-4.5 w-4.5 text-white" fill="currentColor">
                <rect x="2" y="2" width="16" height="4" rx="1" opacity="0.9" />
                <rect x="2" y="8" width="4" height="4" rx="0.5" opacity="0.7" />
                <rect x="8" y="8" width="4" height="4" rx="0.5" opacity="0.7" />
                <rect x="14" y="8" width="4" height="4" rx="0.5" opacity="0.7" />
                <rect x="2" y="14" width="4" height="4" rx="0.5" opacity="0.5" />
                <rect x="8" y="14" width="4" height="4" rx="0.5" opacity="0.5" />
                <rect x="14" y="14" width="4" height="4" rx="0.5" opacity="0.5" />
              </svg>
            </div>
            <span className="text-lg font-bold tracking-tight text-white">
              Dock<span className="text-brand-400">Panel</span>
            </span>
          </Link>
          <Link to="/" className="text-sm text-surface-200/60 transition hover:text-white">
            &larr; Back to home
          </Link>
        </div>
      </header>

      <main className="mx-auto max-w-3xl px-6 py-16">
        <h1 className="text-3xl font-bold tracking-tight text-white md:text-4xl">Privacy Policy</h1>
        <p className="mt-2 text-sm text-surface-200/40">Last updated: March 13, 2026</p>

        <div className="mt-10 space-y-8 text-surface-200/70 leading-relaxed">
          <section>
            <h2 className="text-lg font-semibold text-white">Overview</h2>
            <p className="mt-2">
              DockPanel is a free, open-source, self-hosted server management panel. We are committed to
              transparency about data practices. The short version: we collect almost nothing, and your
              server data never leaves your infrastructure.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">Marketing Website (dockpanel.dev)</h2>
            <p className="mt-2">
              This marketing website does not use cookies, analytics, tracking pixels, or any third-party
              scripts. We do not collect personal information from visitors. No data is stored about your
              visit.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">DockPanel Software (Self-Hosted)</h2>
            <p className="mt-2">
              When you install DockPanel on your own server, all data stays on your server. DockPanel does
              not phone home, send telemetry, or transmit any information to us or third parties. Your
              server configuration, site data, database credentials, and backups remain entirely under your
              control.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">Cookies</h2>
            <p className="mt-2">
              This website does not use cookies. The self-hosted DockPanel application uses a session cookie
              solely for authentication purposes on your own server — this cookie is never shared with any
              external service.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">Third-Party Services</h2>
            <p className="mt-2">
              This website is served via Cloudflare, which may process standard connection metadata (IP
              address, request headers) as part of CDN delivery and DDoS protection. Please refer to{' '}
              <a
                href="https://www.cloudflare.com/privacypolicy/"
                className="text-brand-400 underline underline-offset-2 transition hover:text-brand-300"
                target="_blank"
                rel="noopener noreferrer"
              >
                Cloudflare's Privacy Policy
              </a>{' '}
              for details.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">GitHub</h2>
            <p className="mt-2">
              DockPanel's source code is hosted on GitHub. If you open issues, submit pull requests, or
              interact with the repository, GitHub's privacy policy applies to those interactions.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">Contact</h2>
            <p className="mt-2">
              If you have questions about this privacy policy, you can open an issue on our{' '}
              <a
                href="https://github.com/ovexro/dockpanel"
                className="text-brand-400 underline underline-offset-2 transition hover:text-brand-300"
                target="_blank"
                rel="noopener noreferrer"
              >
                GitHub repository
              </a>
              .
            </p>
          </section>
        </div>
      </main>
    </div>
  );
}
