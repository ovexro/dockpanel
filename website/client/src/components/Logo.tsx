/**
 * The DockPanel mark.
 *
 * There were two marks before this, which is one too many: the header used a
 * lucide `Server` glyph inside a white rounded square — a stock icon in the
 * default container — while the favicon was a different drawing entirely
 * (green ground, three rules, two status squares). Neither was used on the
 * other's surface, so the product had no consistent identity anywhere.
 *
 * This one is drawn from the page's own vocabulary rather than from an icon
 * set. The block is the terminal cursor the install animation already blinks,
 * and the notch cut out of it is a rack unit seen end-on — the two things this
 * product actually is, in one glyph. No enclosing chip: a mark that needs a
 * rounded square behind it to look intentional is not carrying its own weight.
 *
 * The green is the single accent on an otherwise monochrome site, spent in
 * exactly one place. It is the colour the favicon already used, so this
 * unifies the identity rather than inventing a third one.
 */
export function Logo({ className = 'h-6 w-6' }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className={className}
      role="img"
      aria-label="DockPanel"
      fill="none"
    >
      {/* The block. One solid form — the product is one file. */}
      <path
        d="M4 2h16v20H4z"
        fill="#43ef82"
      />
      {/* The slot. A single unit pulled out of the stack, which is also the
          gap in a caret. Cut from the block rather than drawn on top of it,
          so the mark stays legible when it is 16px in a browser tab. */}
      <path
        d="M8 13h8v3H8z"
        fill="#0a0a0b"
      />
    </svg>
  );
}
