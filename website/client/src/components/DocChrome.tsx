import type { ReactNode } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { Logo } from './Logo';

/**
 * The chrome every document page on this site shares.
 *
 * s278 fixed Privacy and Terms by giving them one header instead of two, on the
 * reasoning that a page which hand-rolls its own chrome is a page that will
 * quietly go stale — and both had, for four months, still drawing a mark the
 * product had retired. That fix stopped one file short. `Security.tsx` was
 * rebuilt twenty-nine minutes before the spine landed in `Landing.tsx` and so
 * inherited none of it: its own header, its own ground (#09090b against the
 * token's #0a0a0b), a pill for an eyebrow, and no footer at all. Clearing it by
 * mtime said when it was touched, not what it was built on.
 *
 * So the chrome moves up one level, out of `LegalDoc` and into here, where a
 * document that is not a legal document can have it too. Three pages, one
 * header, one footer, one link treatment. Nothing left on any of them to drift.
 *
 * The layout is the one `index.css` documents and does not get re-invented per
 * page: a hairline between sections (`.rule`) and a fixed left column carrying
 * a mono label that names what KIND of thing the section is (`.measure` +
 * `.eyebrow`). What the label buys on a long reference page is the same thing
 * it buys on a contract — you find the part you came for without a table of
 * contents bolted on top.
 */

/** Body links. Findable without a hue: the underline carries it. */
export const docLink =
  'text-[#f4f4f5] underline decoration-[#3f3f46] underline-offset-2 ' +
  'transition-colors hover:decoration-[#a3a3ad]';

const FOOTER_LINKS = [
  { to: '/privacy', label: 'Privacy' },
  { to: '/terms', label: 'Terms' },
  { to: '/security', label: 'Security' },
];

/**
 * Static, unlike the landing page's: that nav reacts to scroll because it has
 * anchors to track, and a document with none would just be borrowing the motion.
 */
export function DocHeader() {
  return (
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
  );
}

export function DocFooter() {
  const { pathname } = useLocation();

  return (
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
                    here ? 'text-[#a3a3ad]' : 'transition-colors hover:text-zinc-300'
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
          <span className="text-[11px] text-zinc-700">Solo-developed &middot; AGPL v3</span>
        </div>
      </div>
    </footer>
  );
}

/** Ground, header, footer. A page supplies only what sits between them. */
export function DocShell({ children }: { children: ReactNode }) {
  return (
    <div className="min-h-screen bg-[#0a0a0b] text-[#a3a3ad] selection:bg-white/15 selection:text-white">
      <DocHeader />
      {children}
      <DocFooter />
    </div>
  );
}

/**
 * The opening block. No `.rule` above it — the header's own border is already
 * that line, and two hairlines a row apart read as a mistake.
 */
export function DocTitle({
  eyebrow,
  title,
  standfirst,
  meta,
}: {
  eyebrow: string;
  title: string;
  standfirst: ReactNode;
  /** A figure, so it is set in the instrument voice. Omitted when there isn't one. */
  meta?: ReactNode;
}) {
  return (
    <section className="py-16 lg:py-20">
      <div className="measure mx-auto max-w-5xl px-6">
        <p className="eyebrow lg:pt-2">{eyebrow}</p>
        <div className="min-w-0 max-w-2xl">
          <h1 className="text-[2rem] font-semibold leading-[1.15] tracking-[-0.02em] text-[#f4f4f5] sm:text-[2.5rem]">
            {title}
          </h1>
          <p className="mt-4 text-[15px] leading-relaxed text-[#a3a3ad]">{standfirst}</p>
          {meta && <p className="mono tnum mt-6 text-[12px] text-[#3f3f46]">{meta}</p>}
        </div>
      </div>
    </section>
  );
}

/**
 * One section on the spine: the hairline, the margin label, and a content
 * column beside it. What goes in the column is the page's business — prose on a
 * contract, ruled measurements on the security page — so this supplies the
 * structure and nothing else.
 */
export function DocSection({
  id,
  label,
  contentClassName = 'max-w-2xl',
  children,
}: {
  id?: string;
  label: string;
  contentClassName?: string;
  children: ReactNode;
}) {
  return (
    <section id={id} className="rule scroll-mt-4 py-12 lg:py-14">
      <div className="measure mx-auto max-w-5xl px-6">
        <p className="eyebrow lg:pt-1.5">{label}</p>
        <div className={`min-w-0 ${contentClassName}`}>{children}</div>
      </div>
    </section>
  );
}
