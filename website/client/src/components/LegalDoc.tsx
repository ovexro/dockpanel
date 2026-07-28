import type { ReactNode } from 'react';
import { DocSection, DocShell, DocTitle } from './DocChrome';

/**
 * The layout for a legal document: a title block, then clauses.
 *
 * Privacy and Terms drifted for four months, and the reason they drifted is
 * that each one hand-rolled its own header, its own mark and its own link
 * colour. Two copies of a header are two things to remember; when the site was
 * redrawn, both were forgotten, and they kept serving a lucide `Server` glyph
 * in a green square long after that mark had been retired. So the fix was not
 * to restyle two pages — it was to leave them with no chrome of their own to go
 * stale.
 *
 * The chrome itself now lives one level up, in [[DocChrome]], because the
 * security page needed exactly the same header, footer and spine and is not a
 * legal document. What remains here is only the part that IS specific to a
 * contract: a numbered run of clauses, each with a mono margin label naming
 * what kind of clause it is, which is how you find the one you came for
 * without a table of contents bolted on top.
 */

export type LegalSection = {
  /** Anchor target, so a clause can be linked to directly. */
  id: string;
  /** The mono margin label. What kind of thing this is, not a restatement. */
  label: string;
  title: string;
  body: ReactNode;
};

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
  return (
    <DocShell>
      <DocTitle
        eyebrow={eyebrow}
        title={title}
        standfirst={standfirst}
        meta={`Last updated ${updated}`}
      />

      {sections.map((s) => (
        <DocSection key={s.id} id={s.id} label={s.label}>
          <h2 className="text-xl font-semibold leading-snug tracking-[-0.015em] text-[#f4f4f5] sm:text-[1.375rem]">
            {s.title}
          </h2>
          <div className="mt-3 space-y-3 text-[15px] leading-relaxed text-[#a3a3ad]">
            {s.body}
          </div>
        </DocSection>
      ))}
    </DocShell>
  );
}
