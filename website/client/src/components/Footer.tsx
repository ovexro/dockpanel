import { Link } from 'react-router-dom';

export default function Footer() {
  return (
    <footer className="border-t border-white/5 py-12">
      <div className="mx-auto max-w-6xl px-6">
        <div className="grid grid-cols-1 gap-8 md:grid-cols-4">
          {/* Brand */}
          <div className="flex flex-col gap-3">
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
            <p className="text-xs text-surface-200/40 leading-relaxed">
              Free, self-hosted, Docker-native server management panel built in Rust.
            </p>
          </div>

          {/* Product */}
          <div>
            <h3 className="text-xs font-semibold uppercase tracking-wider text-surface-200/40">Product</h3>
            <nav className="mt-3 flex flex-col gap-2">
              <a href="#features" className="text-sm text-surface-200/60 transition hover:text-white">Features</a>
              <a href="#pricing" className="text-sm text-surface-200/60 transition hover:text-white">Pricing</a>
              <a href="#faq" className="text-sm text-surface-200/60 transition hover:text-white">FAQ</a>
              <a href="https://panel.example.com" className="text-sm text-surface-200/60 transition hover:text-white">Demo</a>
            </nav>
          </div>

          {/* Community */}
          <div>
            <h3 className="text-xs font-semibold uppercase tracking-wider text-surface-200/40">Community</h3>
            <nav className="mt-3 flex flex-col gap-2">
              <a href="https://github.com/ovexro/dockpanel" className="text-sm text-surface-200/60 transition hover:text-white">GitHub</a>
              <a href="https://github.com/ovexro/dockpanel#readme" className="text-sm text-surface-200/60 transition hover:text-white">Docs</a>
              <a href="https://github.com/ovexro/dockpanel/releases" className="text-sm text-surface-200/60 transition hover:text-white">Changelog</a>
            </nav>
          </div>

          {/* Legal */}
          <div>
            <h3 className="text-xs font-semibold uppercase tracking-wider text-surface-200/40">Legal</h3>
            <nav className="mt-3 flex flex-col gap-2">
              <Link to="/privacy" className="text-sm text-surface-200/60 transition hover:text-white">Privacy Policy</Link>
              <Link to="/terms" className="text-sm text-surface-200/60 transition hover:text-white">Terms of Service</Link>
            </nav>
          </div>
        </div>

        <div className="mt-10 border-t border-white/5 pt-6">
          <p className="text-center text-xs text-surface-200/40">
            &copy; {new Date().getFullYear()} DockPanel. Free &amp; open source under the MIT License.
          </p>
        </div>
      </div>
    </footer>
  );
}
