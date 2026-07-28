import type { ReactNode } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { Logo } from './Logo';

/**
 * The chrome for a legal document.
 *
 * Privacy and Terms drifted for four months, and the reason they drifted is
 * that each one hand-rolled its own header, its own mark and its own link
 * colour. Two copies of a header are two things to remember; when the site was
 * redrawn, both were forgotten, and they kept serving a lucide `Server` glyph
 * in a green square long after that mark had been retired. So the fix is not
 * to restyle two pages — it is to leave them with no chrome of their own to go
 * stale. A page below supplies sections; everything around the sections lives
 * here, once.
 *
 * The layout is the one index.css describes rather than a third invention: the
 * hairline between sections, and the fixed left column carrying a mono label
 * that says what KIND of thing the section is. On a legal document that margin
 * column earns its place twice over — it is how you find the clause you came
 * for without a table of contents bolted on top.
 *
 * No accent hue, per the same note. The old pages spent green on every link,
 * which is the one colour this palette reserves for the terminal reporting
 * success.
 */

export type LegalSection = {
  /** Anchor target, so a clause can be linked to directly. */
  id: string;
  /** The mono margin label. What kind of thing this is, not a restatement. */
  label: string;
  title: string;
  body: ReactNode;
};

/** Body links. Findable without a hue: the underline carries it. */
export const legalLink =
  'text-[#f4f4f5] underline decoration-[#3f3f46] underline-offset-2 ' +
  'transition-colors hover:decoration-[#a3a3ad]';

const FOOTER_LINKS = [
  { to: '/privacy', label: 'Privacy' },
  { to: '/terms', label: 'Terms' },
  { to: '/security', label: 'Security' },
];

export function LegalDoc({
  eyebrow,
  title,
  standfirst,
  updated,
  sections,
}: {
  eyebrow: string;
  title: string;
  standfirst: ReactNode;
  updated: string;
  sections: LegalSection[];
}) {
  const { pathname } = useLocation();

  return (
    <div className="min-h-screen bg-[#0a0a0b] text-[#a3a3ad] selection:bg-white/15 selection:text-white">
      {/* ── Header ───────────────────────────────────────────────────
          Static, unlike the landing page's: that nav reacts to scroll
          because it has anchors to track, and a document with none would
          just be borrowing the motion. */}
      <header className="border-b border-[#1e1e22]">
        <div className="mx-auto flex h-14 max-w-5xl items-center justify-between px-6">
          <Link to="/" className="flex items-center gap-2.5">
            <Logo className="h-6 w-6" />
            <span className="text-[17px] font-semibold tracking-[-0.01em] text-[#f4f4f5]">
              DockPanel
            </span>
          </Link>
          <Link
            to="/"
            className="text-[13px] text-zinc-500 transition-colors hover:text-white"
          >
            &larr; Back to home
          </Link>
        </div>
      </header>

      {/* ── Title ────────────────────────────────────────────────── */}
      <section className="py-16 lg:py-20">
        <div className="measure mx-auto max-w-5xl px-6">
          <p className="eyebrow lg:pt-2">{eyebrow}</p>
          <div className="max-w-2xl">
            <h1 className="text-[2rem] font-semibold leading-[1.15] tracking-[-0.02em] text-[#f4f4f5] sm:text-[2.5rem]">
              {title}
            </h1>
            <p className="mt-4 text-[15px] leading-relaxed text-[#a3a3ad]">{standfirst}</p>
            {/* A date is a figure, so it is set in the instrument voice. */}
            <p className="mono tnum mt-6 text-[12px] text-[#3f3f46]">
              Last updated {updated}
            </p>
          </div>
        </div>
      </section>

      {/* ── Clauses ──────────────────────────────────────────────── */}
      {sections.map((s) => (
        <section key={s.id} id={s.id} className="rule scroll-mt-4 py-12 lg:py-14">
          <div className="measure mx-auto max-w-5xl px-6">
            <p className="eyebrow lg:pt-1.5">{s.label}</p>
            <div className="max-w-2xl">
              <h2 className="text-xl font-semibold leading-snug tracking-[-0.015em] text-[#f4f4f5] sm:text-[1.375rem]">
                {s.title}
              </h2>
              <div className="mt-3 space-y-3 text-[15px] leading-relaxed text-[#a3a3ad]">
                {s.body}
              </div>
            </div>
          </div>
        </section>
      ))}

      {/* ── Footer ───────────────────────────────────────────────── */}
      <footer className="rule py-10">
        <div className="mx-auto max-w-5xl px-6">
          <div className="flex flex-col gap-6 sm:flex-row sm:items-center sm:justify-between">
            <Link to="/" className="flex items-center gap-2.5">
              <Logo className="h-5 w-5" />
              <span className="text-[14px] font-semibold tracking-[-0.01em] text-[#f4f4f5]">
                DockPanel
              </span>
            </Link>
            <div className="flex flex-wrap gap-x-6 gap-y-2 text-[13px] text-zinc-600">
              {FOOTER_LINKS.map((l) => {
                const here = l.to === pathname;
                return (
                  <Link
                    key={l.to}
                    to={l.to}
                    aria-current={here ? 'page' : undefined}
                    className={
                      here
                        ? 'text-[#a3a3ad]'
                        : 'transition-colors hover:text-zinc-300'
                    }
                  >
                    {l.label}
                  </Link>
                );
              })}
              <a
                href="https://github.com/ovexro/dockpanel"
                className="transition-colors hover:text-zinc-300"
                target="_blank"
                rel="noopener noreferrer"
              >
                GitHub
              </a>
            </div>
          </div>
          <div className="mt-8 flex flex-col items-start justify-between gap-2 border-t border-[#1e1e22] pt-6 sm:flex-row sm:items-center">
            <span className="text-[12px] text-zinc-700">&copy; 2026 DockPanel</span>
            <span className="text-[11px] text-zinc-700">Solo-developed &middot; BSL 1.1</span>
          </div>
        </div>
      </footer>
    </div>
  );
}
