export default function Footer() {
  return (
    <footer className="border-t border-white/5 py-12">
      <div className="mx-auto max-w-6xl px-6">
        <div className="flex flex-col items-center justify-between gap-6 md:flex-row">
          <div className="flex items-center gap-2.5">
            <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-brand-600">
              <svg viewBox="0 0 20 20" className="h-4 w-4 text-white" fill="currentColor">
                <rect x="2" y="2" width="16" height="4" rx="1" opacity="0.9" />
                <rect x="2" y="8" width="4" height="4" rx="0.5" opacity="0.7" />
                <rect x="8" y="8" width="4" height="4" rx="0.5" opacity="0.7" />
                <rect x="14" y="8" width="4" height="4" rx="0.5" opacity="0.7" />
                <rect x="2" y="14" width="4" height="4" rx="0.5" opacity="0.5" />
                <rect x="8" y="14" width="4" height="4" rx="0.5" opacity="0.5" />
                <rect x="14" y="14" width="4" height="4" rx="0.5" opacity="0.5" />
              </svg>
            </div>
            <span className="text-sm font-semibold text-white">
              Dock<span className="text-brand-400">Panel</span>
            </span>
          </div>

          <nav className="flex flex-wrap items-center justify-center gap-6">
            <a href="#features" className="text-sm text-surface-200/60 transition hover:text-white">Features</a>
            <a href="#pricing" className="text-sm text-surface-200/60 transition hover:text-white">Pricing</a>
            <a href="#faq" className="text-sm text-surface-200/60 transition hover:text-white">FAQ</a>
            <a href="https://demo.dockpanel.dev" className="text-sm text-surface-200/60 transition hover:text-white">Demo</a>
            <a href="https://github.com/ovexro/dockpanel" className="text-sm text-surface-200/60 transition hover:text-white">GitHub</a>
          </nav>

          <p className="text-xs text-surface-200/60">
            &copy; {new Date().getFullYear()} DockPanel. Free &amp; open source.
          </p>
        </div>
      </div>
    </footer>
  );
}
