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
  /** Published release assets, linux/amd64, v2.240.0. Verified by the GitHub API. */
  binary: {
    api: 25,
    agent: 22,
    cli: 3.9,
    mcp: 6.1,
    total: 57,
  },

  /**
   * Resident set on a live box, by cgroup accounting
   * (`systemctl show -p MemoryCurrent`), measured 2026-11-25 (v2.242.6, fresh Debian 13, admin + two static sites, idle 6 min, three samples).
   *
   * No CI job can reproduce these — a GitHub runner has no DockPanel install —
   * so they expire instead: the scheduled check fails once the register's date
   * is older than its budget. A measurement nobody has retaken is a claim.
   */
  ram: {
    api: 20,
    agent: 29,
    /** The honest headline: both panel services, nothing else. */
    services: 49,
    /** The number to compare against a competitor's, since theirs include a database. */
    fullStack: 120,
  },

  /**
   * Whole default install, idle, memory in use above a bare Debian 12 box
   * (MemTotal - MemAvailable after a clean reboot, 5 to 9 minutes in), measured
   * 2026-11-25 on identical 8 GB VPSes. Same method for all three, so these are
   * comparable with each other; `ram.fullStack` above is NOT (panel services +
   * PostgreSQL only). Register row: "Claims That Expire" in FEATURES.md.
   */
  idleInstall: {
    dockpanel: 220,
    cloudPanel: 680,
    hestiaCp: 1450,
  },

  /** cPanel and Plesk need a licence: vendor-documented minimum RAM in GB, not a measurement. */
  vendorMinimumGb: {
    cPanel: 2,
    plesk: 1,
  },

  /** Docker app templates in the catalogue. Derived from source by the pin suite. */
  templates: 147,

  /** Install to a working login, Ubuntu 24.04, 1 vCPU. */
  installSeconds: 47,
} as const;
