#!/usr/bin/env node
//
// npm audit with a reviewed allowlist.
//
// `npm audit` has no ignore mechanism, so a single advisory that does not apply
// to us leaves the whole Security Audit job red on every commit. That is worse
// than having no job: a gate that is always failing cannot report a NEW failure,
// and the next real advisory lands invisibly behind a badge nobody looks at any
// more.
//
// So: everything still fails the build, except advisories listed below with a
// written reason. An allowlist entry that no longer matches anything is reported
// too — an entry nothing can hit is how an allowlist quietly turns into a blanket.
//
// Usage:  node scripts/npm-audit-gate.mjs [--input audit.json] [--level high]
//         (run from the directory holding the package.json to audit)
//
// Exit codes, kept distinct because the pre-push hook treats them differently:
//   0  clean, or everything at/above the level is waived
//   1  a real un-waived advisory — block
//   2  the output could not be parsed — block, because we cannot tell
//   3  npm could not reach the advisory source — WARN and let it through. A gate
//      that blocks on a DNS blip is a gate that gets disabled with --no-verify.

import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const ALLOWLIST = [
  // Empty on purpose, and that is the interesting state: nothing is currently
  // being excused.
  //
  // It held GHSA-qwww-vcr4-c8h2 (react-router RSC-mode CSRF bypass) from
  // 2026-07-27. The waiver's reason ended "re-check when react-router ships a fix
  // inside the ^7 range", and `live-surfaces-check.sh` was given the job of
  // watching for exactly that rather than trusting it. On 2026-08-07 it fired:
  // 7.18.2 carries the fix. Both frontends were already on `^7.18.1`, so the fix
  // was one `npm install` inside the range the waiver said would need a major
  // migration — the waiver was accurate when written and stopped being accurate
  // without any commit here changing.
  //
  // ⚠ Retiring a waiver is not just deleting the entry: two checks existed to
  // police THIS waiver and both had to be re-pointed, or they would have gone on
  // asserting that a waiver exists. See `ssl-correctness-pin-e2e.sh` §A and
  // `live-surfaces-check.sh` §4, which now assert the opposite — that the
  // advisory is NOT waived, and that the version we ship is at or above the fix.
];

const RANK = { info: 0, low: 1, moderate: 2, high: 3, critical: 4 };

const args = process.argv.slice(2);
const argOf = (flag) => {
  const i = args.indexOf(flag);
  return i === -1 ? null : args[i + 1];
};
const level = argOf('--level') ?? 'high';
const inputFile = argOf('--input');

let raw;
if (inputFile) {
  raw = readFileSync(inputFile, 'utf8');
} else {
  // npm audit exits non-zero whenever it finds anything, so the exit code says
  // nothing useful here — the report does. Read stdout regardless.
  const res = spawnSync('npm', ['audit', '--json'], {
    encoding: 'utf8',
    maxBuffer: 32 * 1024 * 1024,
  });
  if (res.error) {
    console.error(`npm audit could not be run: ${res.error.message}`);
    process.exit(2);
  }
  raw = res.stdout;
}

let report;
try {
  report = JSON.parse(raw);
} catch {
  console.error('npm audit did not return JSON. Raw output follows:\n' + raw.slice(0, 2000));
  process.exit(2);
}

if (report.error) {
  // npm reports registry/network trouble this way, and it is indistinguishable
  // from a real finding by exit code alone. Distinct code so the caller can
  // decide; the pre-push hook warns, CI blocks.
  console.error(`npm audit could not complete: ${report.error.summary ?? JSON.stringify(report.error)}`);
  process.exit(3);
}

// npm >= 7 report shape: { vulnerabilities: { <pkg>: { via: [ {url, title, severity} | "<pkg>" ] } } }
const advisories = new Map();
for (const node of Object.values(report.vulnerabilities ?? {})) {
  for (const via of node.via ?? []) {
    if (typeof via !== 'object' || !via.url) continue;
    const id = via.url.split('/').filter(Boolean).pop();
    if (!advisories.has(id)) {
      advisories.set(id, {
        id,
        title: via.title ?? '(no title)',
        severity: via.severity ?? 'unknown',
        packages: new Set(),
      });
    }
    advisories.get(id).packages.add(via.name ?? node.name);
  }
}

const threshold = RANK[level] ?? RANK.high;
const allowed = new Map(ALLOWLIST.map((e) => [e.id, e]));
const blocking = [];
const waived = [];

for (const adv of advisories.values()) {
  if ((RANK[adv.severity] ?? 99) < threshold) continue;
  (allowed.has(adv.id) ? waived : blocking).push(adv);
}

const pkgs = (adv) => [...adv.packages].join(', ');

for (const adv of waived) {
  const entry = allowed.get(adv.id);
  console.log(`waived  ${adv.id}  ${adv.severity}  ${pkgs(adv)}`);
  console.log(`        reviewed ${entry.reviewed}: ${entry.reason}`);
}

// An allowlist entry that matches nothing has stopped being a decision and
// started being a hole. Report it; do not fail on it, since it is usually just
// a dependency that got fixed.
for (const entry of ALLOWLIST) {
  if (!advisories.has(entry.id)) {
    console.log(`stale   ${entry.id} is allowlisted but no longer reported here — drop it once no manifest needs it.`);
  }
}

if (blocking.length === 0) {
  console.log(`ok      no un-waived advisories at severity >= ${level}`);
  process.exit(0);
}

console.error(`\nFAIL: ${blocking.length} advisory(ies) at severity >= ${level} with no reviewed waiver:\n`);
for (const adv of blocking) {
  console.error(`  ${adv.id}  ${adv.severity}  ${pkgs(adv)}`);
  console.error(`    ${adv.title}`);
  console.error(`    https://github.com/advisories/${adv.id}`);
}
console.error('\nFix it, or add it to ALLOWLIST in scripts/npm-audit-gate.mjs with a reason that says why it cannot affect this build.');
process.exit(1);
