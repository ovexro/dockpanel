import { useState } from 'react';

export default function Header() {
  const [mobileOpen, setMobileOpen] = useState(false);

  return (
    <header className="fixed top-0 left-0 right-0 z-50 border-b border-white/5 bg-surface-950/80 backdrop-blur-xl">
      <div className="mx-auto flex max-w-6xl items-center justify-between px-6 py-4">
        <a href="/" className="flex items-center gap-2.5">
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
        </a>

        <nav className="hidden items-center gap-8 md:flex">
          <a href="#features" className="text-sm text-surface-200/70 transition hover:text-white">Features</a>
          <a href="#how-it-works" className="text-sm text-surface-200/70 transition hover:text-white">How it works</a>
          <a href="#pricing" className="text-sm text-surface-200/70 transition hover:text-white">Pricing</a>
          <a href="#faq" className="text-sm text-surface-200/70 transition hover:text-white">FAQ</a>
          <a href="https://panel.example.com" className="text-sm text-surface-200/70 transition hover:text-white">Demo</a>
          <a
            href="https://github.com/ovexro/dockpanel"
            className="inline-flex items-center gap-2 rounded-lg bg-brand-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-brand-500"
          >
            <svg className="h-4 w-4" fill="currentColor" viewBox="0 0 24 24"><path d="M12 0C5.374 0 0 5.373 0 12c0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23A11.509 11.509 0 0112 5.803c1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576C20.566 21.797 24 17.3 24 12c0-6.627-5.373-12-12-12z"/></svg>
            GitHub
          </a>
        </nav>

        <button
          onClick={() => setMobileOpen(!mobileOpen)}
          className="flex h-10 w-10 items-center justify-center rounded-lg text-surface-200/70 transition hover:bg-white/5 hover:text-white md:hidden"
          aria-label="Toggle menu"
        >
          {mobileOpen ? (
            <svg className="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          ) : (
            <svg className="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 6h16M4 12h16M4 18h16" />
            </svg>
          )}
        </button>
      </div>

      {mobileOpen && (
        <nav className="border-t border-white/5 bg-surface-950/95 px-6 py-4 backdrop-blur-xl md:hidden">
          <div className="flex flex-col gap-4">
            <a href="#features" onClick={() => setMobileOpen(false)} className="text-sm text-surface-200/70 transition hover:text-white">Features</a>
            <a href="#how-it-works" onClick={() => setMobileOpen(false)} className="text-sm text-surface-200/70 transition hover:text-white">How it works</a>
            <a href="#pricing" onClick={() => setMobileOpen(false)} className="text-sm text-surface-200/70 transition hover:text-white">Pricing</a>
            <a href="#faq" onClick={() => setMobileOpen(false)} className="text-sm text-surface-200/70 transition hover:text-white">FAQ</a>
            <a href="https://panel.example.com" onClick={() => setMobileOpen(false)} className="text-sm text-surface-200/70 transition hover:text-white">Demo</a>
            <a
              href="https://github.com/ovexro/dockpanel"
              onClick={() => setMobileOpen(false)}
              className="mt-2 inline-flex items-center justify-center gap-2 rounded-lg bg-brand-600 px-4 py-2.5 text-center text-sm font-medium text-white transition hover:bg-brand-500"
            >
              <svg className="h-4 w-4" fill="currentColor" viewBox="0 0 24 24"><path d="M12 0C5.374 0 0 5.373 0 12c0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23A11.509 11.509 0 0112 5.803c1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576C20.566 21.797 24 17.3 24 12c0-6.627-5.373-12-12-12z"/></svg>
              GitHub
            </a>
          </div>
        </nav>
      )}
    </header>
  );
}
