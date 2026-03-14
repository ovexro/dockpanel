const panels = [
  { name: 'cPanel', ram: '~800 MB', install: '~45 min', size: '~2 GB', price: '$15/mo', tech: 'Perl/PHP', docker: false, selfHosted: true },
  { name: 'Plesk', ram: '~512 MB', install: '~20 min', size: '~1.5 GB', price: '$10/mo', tech: 'PHP', docker: false, selfHosted: true },
  { name: 'RunCloud', ram: 'N/A (SaaS)', install: '~5 min', size: 'Agent', price: '$8/mo', tech: 'PHP', docker: false, selfHosted: false },
  { name: 'CloudPanel', ram: '~250 MB', install: '~10 min', size: '~600 MB', price: 'Free', tech: 'PHP', docker: false, selfHosted: true },
  { name: 'DockPanel', ram: '12 MB', install: '<60 sec', size: '~10 MB', price: 'Free', tech: 'Rust', docker: true, selfHosted: true, highlight: true },
];

const specs = [
  { key: 'ram', label: 'RAM Usage' },
  { key: 'install', label: 'Install Time' },
  { key: 'size', label: 'Disk Size' },
  { key: 'price', label: 'Price' },
  { key: 'tech', label: 'Built With' },
] as const;

function BoolBadge({ value }: { value: boolean }) {
  return value ? (
    <span className="inline-flex h-6 w-6 items-center justify-center rounded-full bg-green-500/20 text-green-400">
      <svg className="h-3.5 w-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2.5} d="M5 13l4 4L19 7" />
      </svg>
    </span>
  ) : (
    <span className="inline-flex h-6 w-6 items-center justify-center rounded-full bg-red-500/15 text-red-400/60">
      <svg className="h-3.5 w-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2.5} d="M6 18L18 6M6 6l12 12" />
      </svg>
    </span>
  );
}

export default function Comparison() {
  return (
    <section className="section-alt py-24">
      <div className="mx-auto max-w-6xl px-6">
        <div className="text-center">
          <p className="text-sm font-medium uppercase tracking-wider text-brand-400">Comparison</p>
          <h2 className="mt-3 text-3xl font-bold tracking-tight text-white md:text-4xl">
            See the difference
          </h2>
          <p className="mx-auto mt-4 max-w-2xl text-surface-200/60">
            Traditional panels were built for bare metal servers in 2005.
            SaaS panels lock you in with monthly fees.
            DockPanel is free, self-hosted, and built for modern infrastructure.
          </p>
        </div>

        {/* Desktop table */}
        <div className="mt-12 hidden md:block">
          <div className="overflow-hidden rounded-2xl border border-white/[0.06] bg-white/[0.015]">
            <table className="w-full text-left text-sm">
              <thead>
                <tr className="border-b border-white/[0.08]">
                  <th scope="col" className="py-4 px-6 font-medium text-surface-200/50">Panel</th>
                  <th scope="col" className="px-4 py-4 font-medium text-surface-200/50">RAM Usage</th>
                  <th scope="col" className="px-4 py-4 font-medium text-surface-200/50">Install Time</th>
                  <th scope="col" className="px-4 py-4 font-medium text-surface-200/50">Disk Size</th>
                  <th scope="col" className="px-4 py-4 font-medium text-surface-200/50">Price</th>
                  <th scope="col" className="px-4 py-4 font-medium text-surface-200/50">Built With</th>
                  <th scope="col" className="px-4 py-4 font-medium text-surface-200/50">Docker-Native</th>
                  <th scope="col" className="px-4 py-4 font-medium text-surface-200/50">Self-Hosted</th>
                </tr>
              </thead>
              <tbody>
                {panels.map((p) => (
                  <tr
                    key={p.name}
                    className={`border-b border-white/[0.04] transition-colors last:border-b-0 ${
                      p.highlight
                        ? 'bg-brand-500/[0.06]'
                        : 'hover:bg-white/[0.015]'
                    }`}
                  >
                    <td className={`py-4 px-6 font-medium ${p.highlight ? 'text-brand-300' : 'text-white'}`}>
                      {p.name}
                    </td>
                    <td className={`px-4 py-4 ${p.highlight ? 'text-brand-200 font-semibold' : 'text-surface-200/50'}`}>{p.ram}</td>
                    <td className={`px-4 py-4 ${p.highlight ? 'text-brand-200 font-semibold' : 'text-surface-200/50'}`}>{p.install}</td>
                    <td className={`px-4 py-4 ${p.highlight ? 'text-brand-200 font-semibold' : 'text-surface-200/50'}`}>{p.size}</td>
                    <td className={`px-4 py-4 ${p.highlight ? 'text-brand-200 font-semibold' : 'text-surface-200/50'}`}>{p.price}</td>
                    <td className={`px-4 py-4 ${p.highlight ? 'text-brand-200 font-semibold' : 'text-surface-200/50'}`}>{p.tech}</td>
                    <td className="px-4 py-4"><BoolBadge value={p.docker} /></td>
                    <td className="px-4 py-4"><BoolBadge value={p.selfHosted} /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>

        {/* Mobile cards */}
        <div className="mt-12 space-y-4 md:hidden">
          {panels.map((p) => (
            <div
              key={p.name}
              className={`rounded-2xl border p-5 ${
                p.highlight
                  ? 'border-brand-500/30 bg-brand-500/[0.06]'
                  : 'border-white/[0.06] bg-white/[0.015]'
              }`}
            >
              <div className={`text-lg font-semibold ${p.highlight ? 'text-brand-300' : 'text-white'}`}>
                {p.name}
              </div>
              <div className="mt-3 grid grid-cols-2 gap-y-2.5 gap-x-4 text-sm">
                {specs.map(({ key, label }) => (
                  <div key={key}>
                    <div className="text-[11px] uppercase tracking-wider text-surface-200/30">{label}</div>
                    <div className={p.highlight ? 'font-semibold text-brand-200' : 'text-surface-200/60'}>
                      {p[key]}
                    </div>
                  </div>
                ))}
              </div>
              <div className="mt-3 flex items-center gap-4 text-sm">
                <div className="flex items-center gap-1.5">
                  <BoolBadge value={p.docker} />
                  <span className="text-surface-200/40">Docker</span>
                </div>
                <div className="flex items-center gap-1.5">
                  <BoolBadge value={p.selfHosted} />
                  <span className="text-surface-200/40">Self-Hosted</span>
                </div>
              </div>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
