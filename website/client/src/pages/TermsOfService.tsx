import { Link } from 'react-router-dom';
import { Server } from 'lucide-react';

export default function TermsOfService() {
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
        <h1 className="text-3xl font-bold tracking-tight text-white md:text-4xl">Terms of Service</h1>
        <p className="mt-2 text-sm text-zinc-500">Last updated: March 13, 2026</p>

        <div className="mt-10 space-y-8 text-zinc-400 leading-relaxed">
          <section>
            <h2 className="text-lg font-semibold text-white">Overview</h2>
            <p className="mt-2">
              DockPanel is free, open-source software licensed under the{' '}
              <a
                href="https://opensource.org/licenses/MIT"
                className="text-emerald-400 underline underline-offset-2 transition hover:text-emerald-300"
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
                className="text-emerald-400 underline underline-offset-2 transition hover:text-emerald-300"
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
              A demo may be available upon request for evaluation purposes.
            </p>
          </section>

          <section>
            <h2 className="text-lg font-semibold text-white">Support</h2>
            <p className="mt-2">
              Community support is available through{' '}
              <a
                href="https://github.com/ovexro/dockpanel/issues"
                className="text-emerald-400 underline underline-offset-2 transition hover:text-emerald-300"
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
