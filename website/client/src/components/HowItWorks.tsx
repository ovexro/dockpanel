const steps = [
  {
    step: '01',
    title: 'Install with one command',
    description: 'Run the install script on any fresh VPS. DockPanel detects your OS, installs Docker if needed, and sets everything up automatically.',
    code: 'curl -sL https://dockpanel.dev/install.sh | sudo bash',
    isTerminal: true,
  },
  {
    step: '02',
    title: 'Create your account',
    description: 'Open your browser and navigate to your server\'s panel URL. Complete the one-time setup wizard to create your admin account.',
    code: 'https://panel.your-domain.com',
    isTerminal: false,
  },
  {
    step: '03',
    title: 'Add your first site',
    description: 'Click "New Site" in the dashboard, enter your domain name, and DockPanel configures Nginx, provisions a free SSL certificate, and gets your site live.',
    code: 'Dashboard  →  Sites  →  New Site  →  example.com',
    isTerminal: false,
  },
];

export default function HowItWorks() {
  return (
    <section id="how-it-works" className="py-24">
      <div className="mx-auto max-w-6xl px-6">
        <div className="text-center">
          <p className="text-sm font-medium uppercase tracking-wider text-brand-400">How it works</p>
          <h2 className="mt-3 text-3xl font-bold tracking-tight text-white md:text-4xl">
            Up and running in under 60 seconds
          </h2>
        </div>

        <div className="mt-16 space-y-12">
          {steps.map(({ step, title, description, code, isTerminal }, i) => (
            <div
              key={step}
              className={`flex flex-col items-center gap-8 md:flex-row ${
                i % 2 === 1 ? 'md:flex-row-reverse' : ''
              }`}
            >
              <div className="flex-1">
                <div className="inline-flex items-center gap-3">
                  <span className="text-4xl font-extrabold text-brand-400/60">{step}</span>
                  <h3 className="text-xl font-semibold text-white">{title}</h3>
                </div>
                <p className="mt-3 text-surface-200/60 leading-relaxed">{description}</p>
              </div>
              <div className="w-full flex-1">
                <div className="overflow-hidden rounded-xl border border-white/[0.08] bg-surface-900/60">
                  {/* Window chrome */}
                  <div className="flex items-center gap-2 border-b border-white/5 px-4 py-2.5">
                    <div className="h-2.5 w-2.5 rounded-full bg-red-500/50" />
                    <div className="h-2.5 w-2.5 rounded-full bg-yellow-500/50" />
                    <div className="h-2.5 w-2.5 rounded-full bg-green-500/50" />
                    {!isTerminal && (
                      <div className="ml-2 flex-1 max-w-[200px]">
                        <div className="rounded bg-white/[0.04] px-2 py-0.5 text-[10px] text-surface-200/25 truncate">
                          {i === 1 ? 'panel.your-domain.com' : 'DockPanel'}
                        </div>
                      </div>
                    )}
                  </div>
                  <div className="p-5">
                    <code className="text-sm text-surface-200/70">
                      {isTerminal && <span className="text-brand-400">$ </span>}
                      {code}
                    </code>
                  </div>
                </div>
              </div>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
