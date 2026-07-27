// Stage scripts/install-agent.sh into public/ so every frontend build re-emits
// it into dist/.
//
// The panel's Add-Server dialog prints `curl -sSL {panel}/install-agent.sh |
// sudo bash -s -- …`, and nginx serves that file straight out of the frontend
// dist. Until s275 the file was only ever PUT there by an installer —
// setup.sh, update.sh or deploy-demo.sh — and never by the build.
//
// Vite empties outDir on every build. So `npm run build`, an ordinary and
// entirely unrelated command, DELETED the installer from the live panel and
// left the Add-Server command returning 200 with SPA fallback HTML until an
// installer happened to run again. s274 read this as "deploy-demo.sh forgot a
// step" and fixed that one path; the real defect is that the artefact lived
// only in a directory that a routine command wipes.
//
// The marketing site had already solved exactly this: website/client/public/
// holds install.sh, so Vite copies it in on every build. This does the same for
// the panel, from the single source in scripts/ rather than a committed copy —
// a second checked-in copy of an installer is the thing this codebase keeps
// getting wrong (see the ReadWritePaths derivation, four copies deep).
//
// The staged file is gitignored. It is a build artefact, not a source file.

import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const src = resolve(repoRoot, "scripts/install-agent.sh");
const destDir = resolve(here, "../public");
const dest = resolve(destDir, "install-agent.sh");

if (!existsSync(src)) {
  // Loudly. A silent skip here reproduces the defect this file exists to close:
  // the build succeeds, the panel serves SPA HTML at that URL, and the operator
  // pipes a web page into sudo bash.
  console.error(`[stage-install-agent] FATAL: ${src} does not exist.`);
  console.error("[stage-install-agent] The Add-Server command would serve SPA fallback HTML.");
  process.exit(1);
}

mkdirSync(destDir, { recursive: true });
copyFileSync(src, dest);
console.log(`[stage-install-agent] staged ${src} -> ${dest}`);
