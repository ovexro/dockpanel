import { Link } from 'react-router-dom';
import { Server } from 'lucide-react';

export default function PrivacyPolicy() {
  return (
    <div className="min-h-screen">
      {/* Simple header */}
      <header className="border-b border-zinc-800 bg-zinc-950/80 backdrop-blur-xl">
        <div className="mx-auto flex max-w-4xl items-center justify-between px-6 py-4">
          <Link to="/" className="flex items-center gap-2.5">
            <div className="w-8 h-8 rounded-lg bg-emerald-500 flex items-center justify-center">
              <Server className="w-5 h-5 text-zinc-950" />
            </div>
            <span className="text-lg font-bold tracking-tight text-white">
              Dock<span className="text-emerald-400">Panel</span>
            </span>
          </Link>
          <Link to="/" className="text-sm text-zinc-400 transition hover:text-white">
            &larr; Back to home
          </Link>
        </div>
      </header>

      <main className="mx-auto max-w-3xl px-6 py-16">
        <h1 className="text-3xl font-bold tracking-tight text-white md:text-4xl">Privacy Policy</h1>
        <p className="mt-2 text-sm text-zinc-500">Last updated: March 13, 2026</p>

        <div className="mt-10 space-y-8 text-zinc-400 leading-relaxed">
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
                className="text-emerald-400 underline underline-offset-2 transition hover:text-emerald-300"
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
                className="text-emerald-400 underline underline-offset-2 transition hover:text-emerald-300"
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
