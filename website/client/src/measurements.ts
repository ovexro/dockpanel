/**
 * Every figure this site publishes about DockPanel, in one place.
 *
 * The reason this file exists: s271 audited every number on the landing page
 * and corrected six of them, and s272 found three more still wrong in the same
 * file — including two entries of the same FAQ list that gave different answers
 * for the same measurement, three lines apart. Hand-auditing a page of numbers
 * does not converge, because the thing that rots is not any single figure, it
 * is having the figure written down in more than one place.
 *
 * So the page quotes; it does not state. A contradiction between two parts of
 * this site is now impossible to write, rather than merely unlikely to survive
 * review.
 *
 * These values mirror the measurement register in FEATURES.md, which is the
 * project-wide source of truth across the README, the docs and this site.
 * `tests/docs-claims-pin-e2e.sh` fails the build if this file and that table
 * ever disagree, so edit the register first.
 *
 * MB means MiB (2^20 bytes) — the unit `ls -lh`, `free -h` and `docker stats`
 * print. The page used to quote binary sizes in that convention and memory
 * figures in decimal, which is its own quiet way of being wrong.
 */

export const measurements = {
  /** Published release assets, linux/amd64, v2.44.0. Verified by the GitHub API. */
  binary: {
    api: 22,
    agent: 21,
    cli: 1.7,
    total: 45,
  },

  /**
   * Resident set on a live box, by cgroup accounting
   * (`systemctl show -p MemoryCurrent`), measured 2026-07-27.
   *
   * No CI job can reproduce these — a GitHub runner has no DockPanel install —
   * so they expire instead: the scheduled check fails once the register's date
   * is older than its budget. A measurement nobody has retaken is a claim.
   */
  ram: {
    api: 14,
    agent: 35,
    /** The honest headline: both panel services, nothing else. */
    services: 49,
    /** The number to compare against a competitor's, since theirs include a database. */
    fullStack: 109,
  },

  /** Competitors, whole stack against whole stack. See the note in Landing.tsx. */
  competitorRam: {
    cloudPanel: 250,
    plesk: 512,
    hestiaCp: 512,
    cPanel: 800,
  },

  /** Docker app templates in the catalogue. Derived from source by the pin suite. */
  templates: 149,

  /** Install to a working login, Ubuntu 24.04, 1 vCPU. */
  installSeconds: 47,
} as const;
