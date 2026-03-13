import { Link } from 'react-router-dom';

export default function TermsOfService() {
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
        <h1 className="text-3xl font-bold tracking-tight text-white md:text-4xl">Terms of Service</h1>
        <p className="mt-2 text-sm text-surface-200/40">Last updated: March 13, 2026</p>

        <div className="mt-10 space-y-8 text-surface-200/70 leading-relaxed">
          <section>
            <h2 className="text-lg font-semibold text-white">Overview</h2>
            <p className="mt-2">
              DockPanel is free, open-source software licensed under the{' '}
              <a
                href="https://opensource.org/licenses/MIT"
                className="text-brand-400 underline underline-offset-2 transition hover:text-brand-300"
                target="_blank"
                rel="noopener noreferrer"
              >
                MIT License
              </a>
              . By using DockPanel, you agree to the following terms.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">License</h2>
            <p className="mt-2">
              DockPanel is released under the MIT License. You are free to use, copy, modify, merge,
              publish, distribute, sublicense, and/or sell copies of the software, subject to the conditions
              of the MIT License. The full license text is available in the{' '}
              <a
                href="https://github.com/ovexro/dockpanel/blob/main/LICENSE"
                className="text-brand-400 underline underline-offset-2 transition hover:text-brand-300"
                target="_blank"
                rel="noopener noreferrer"
              >
                GitHub repository
              </a>
              .
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">No Warranty</h2>
            <p className="mt-2">
              THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
              BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
              NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
              DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
              OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">Your Responsibility</h2>
            <p className="mt-2">
              DockPanel is self-hosted software that you install and run on your own server. You are solely
              responsible for:
            </p>
            <ul className="mt-3 list-disc space-y-1.5 pl-5">
              <li>The security and maintenance of your server</li>
              <li>Keeping DockPanel and its dependencies up to date</li>
              <li>Backing up your data and configurations</li>
              <li>Compliance with applicable laws and regulations in your jurisdiction</li>
              <li>Any content hosted on servers managed by DockPanel</li>
            </ul>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">Demo Service</h2>
            <p className="mt-2">
              We provide a demo instance at demo.dockpanel.dev for evaluation purposes. This demo is
              provided as-is with no uptime guarantees. Do not enter real credentials or sensitive data into
              the demo.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">Support</h2>
            <p className="mt-2">
              Community support is available through{' '}
              <a
                href="https://github.com/ovexro/dockpanel/issues"
                className="text-brand-400 underline underline-offset-2 transition hover:text-brand-300"
                target="_blank"
                rel="noopener noreferrer"
              >
                GitHub Issues
              </a>
              . While we strive to help, community support is provided on a best-effort basis with no
              guaranteed response times.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">Changes to Terms</h2>
            <p className="mt-2">
              We may update these terms from time to time. Changes will be reflected on this page with an
              updated date. Continued use of DockPanel after changes constitutes acceptance of the new terms.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">Contact</h2>
            <p className="mt-2">
              For questions about these terms, please open an issue on our{' '}
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
