#!/usr/bin/env bash
# Regression pins for the s457 dockpanel-fanout run over
# panel/agent/src/services/image_scanner.rs + routes/image_scan.rs (workflow
# wf_6a5a4e25-9ee; 2 topics finder/skeptic + completeness/setup critics).
#
#   S1  services/image_scanner.rs::install_grype fetched Anchore's install.sh
#       from the mutable `main` ref and let it resolve "latest" at install
#       time — no tag pinned. (Anchore's install.sh ALREADY sha256-verifies
#       the downloaded archive against its own release checksums.txt
#       unconditionally — read upstream to confirm this session, not assumed
#       — so the gap was reproducibility/pinning, not missing integrity
#       verification.) Fixed: both the script fetch and the installed
#       release are pinned to GRYPE_VERSION via install.sh's own TAG arg.
#   S2  services/sbom_scanner.rs::install_syft — identical shape, same fix,
#       pinned to SYFT_VERSION.
#   S3  Neither scanner's process spawns set kill_on_drop — a timed-out
#       grype/syft child outlives the 180s the caller is told it "timed
#       out", the exact kill_on_drop/timeout-orphan class this project has
#       already fixed five times (docker_apps.rs, pkg.rs, database_backup.rs,
#       wp_vulnerability.rs, backups.rs — s446-453); image_scanner.rs and
#       sbom_scanner.rs were never swept in that arc. Fixed on all 5 spawn
#       sites across both files (2 install spawns + grype's db-update-priming
#       spawn + the two per-scan spawns).
#
# Completeness-critic finding, off the original topic menu:
#   S4  services/wordpress.rs::install and 4 of cms.rs's 5 CMS installers
#       (Laravel/Drupal/Joomla/CodeIgniter — Symfony's install() takes no DB
#       credentials at all, confirmed by its own signature, so it is
#       deliberately NOT wired) wrote a DB-credential-bearing config file
#       (wp-config.php / .env / settings.php / configuration.php) at the
#       process umask's default 644, world-readable, with no chmod anywhere
#       on the install path. wp_vulnerability.rs's own "wp-config-perms"
#       hardening check (chmod 640) already existed but only ran via the
#       separate opt-in POST /wordpress/{domain}/harden pass, never wired
#       into install() itself.
#       ⚠ HONEST SCOPE, live-verified this session (chmod 600/640 vs a
#       same-UID reader on this box: both still readable; only a genuinely
#       different UID was denied — see the fix's own comment in cms.rs and
#       wordpress.rs): this closes reads from any OTHER local OS account.
#       It does NOT and structurally CANNOT isolate one tenant's own
#       www-data-identity PHP-FPM pool or client-role shell from another
#       tenant's credential file, since every site currently shares the
#       identical www-data:www-data identity (nginx.rs hardcodes
#       user=www-data/group=www-data for every pool, and no open_basedir
#       anywhere restricts a pool's own filesystem view). That is the
#       separate, already-tracked per-site-OS-user isolation gap (#64/#85 in
#       project_dockpanel_tech_debt.md), unchanged by this fix — do not cite
#       this pin as evidence it closed cross-tenant isolation.
#
# Deliberately NOT fixed this session (filed as carries, not pinned here):
#   - SSRF-via-arbitrary-registry-host in scan_image()/generate_sbom() —
#     live-verified real (grype opens its own outbound HTTPS connection to
#     an attacker-shaped host:port, no Docker daemon involved) but a naive
#     RFC1918/loopback denylist would break a legitimate self-hosted
#     private-registry use case; needs an explicit allowlist design.
#   - pkg.rs's NodeSource curl|bash installer — same install-script-execution
#     shape but version-scoped in its own URL already and lower priority per
#     the setup critic.
#
# Pure source analysis; no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# Strip comments and the test module. NOT the naive "sed-quit-at-first-
# #[cfg(test)]" shape — see project_dockpanel_lessons_p182 (s456): only exit
# at a #[cfg(test)] line immediately followed by `mod `, a real test module,
# not a cfg-gated function/const variant elsewhere in the file.
code() {
  awk '
    held == 1 {
      if ($0 ~ /^mod /) { exit }
      print heldline
      held = 0
    }
    /^#\[cfg\(test\)\]$/ { held = 1; heldline = $0; next }
    { print }
  ' "$1" | grep -vE '^[[:space:]]*(///|//!|//|\*|/\*)'
}
has()   { [ -n "$(code "$1" | grep -F -- "$2")" ]; }
hasre() { [ -n "$(code "$1" | grep -E -- "$2")" ]; }

# One named function's body, comment-stripped. Function-name patterns MUST be
# anchored (^ ... \() — GNU awk's \b silently matches nothing in default ERE
# mode (project_dockpanel_lessons_p182, s456), and several names here
# (install, install_grype, install_syft) are prefixes of no sibling in these
# files, but anchoring is the standing discipline regardless.
fnbody()    { code "$1" | awk "/$2/,/^}/"; }
bodyhas()   { [ -n "$(fnbody "$1" "$2" | grep -F -- "$3")" ]; }
bodyhasre() { [ -n "$(fnbody "$1" "$2" | grep -E -- "$3")" ]; }
countin()   { fnbody "$1" "$2" | grep -Fc -- "$3"; }

IMAGE_SCANNER=panel/agent/src/services/image_scanner.rs
SBOM_SCANNER=panel/agent/src/services/sbom_scanner.rs
CMS=panel/agent/src/services/cms.rs
WORDPRESS=panel/agent/src/services/wordpress.rs

for f in "$IMAGE_SCANNER" "$SBOM_SCANNER" "$CMS" "$WORDPRESS"; do
  [ -f "$f" ] || { echo "missing source file: $f"; exit 1; }
done

echo "── 1. S1: image_scanner.rs::install_grype — pinned to a tag, not main ──"
if hasre "$IMAGE_SCANNER" '^const GRYPE_VERSION: &str = "v[0-9]'; then
  ok "GRYPE_VERSION is a pinned version-tag constant"
else
  bad "no pinned GRYPE_VERSION constant found"
fi
if bodyhas "$IMAGE_SCANNER" "^pub async fn install_grype" '/main/install.sh'; then
  bad "install_grype still fetches install.sh from the mutable main ref"
else
  ok "install_grype no longer fetches install.sh from /main/"
fi
if bodyhasre "$IMAGE_SCANNER" "^pub async fn install_grype" 'grype/\{GRYPE_VERSION\}/install\.sh'; then
  ok "install_grype fetches install.sh from the pinned GRYPE_VERSION tag ref"
else
  bad "install_grype's install.sh fetch is not pinned to GRYPE_VERSION's ref"
fi
if bodyhasre "$IMAGE_SCANNER" "^pub async fn install_grype" '\-b \{GRYPE_DIR\} \{GRYPE_VERSION\}'; then
  ok "install_grype passes GRYPE_VERSION as install.sh's TAG argument"
else
  bad "install_grype no longer pins the installed release via install.sh's TAG arg"
fi

echo
echo "── 2. S3 (grype half): kill_on_drop on all 3 spawns in image_scanner.rs ──"
if [ "$(countin "$IMAGE_SCANNER" "^pub async fn install_grype" "kill_on_drop(true)")" -ge 2 ]; then
  ok "install_grype sets kill_on_drop(true) on both its spawns (installer + db-update priming)"
else
  bad "install_grype is missing kill_on_drop(true) on one or both of its spawns"
fi
if bodyhas "$IMAGE_SCANNER" "^pub async fn scan_image" "kill_on_drop(true)"; then
  ok "scan_image sets kill_on_drop(true) on the grype invocation"
else
  bad "scan_image is missing kill_on_drop(true) — a timed-out scan can orphan"
fi

echo
echo "── 3. S2: sbom_scanner.rs::install_syft — pinned to a tag, not main ──"
if hasre "$SBOM_SCANNER" '^const SYFT_VERSION: &str = "v[0-9]'; then
  ok "SYFT_VERSION is a pinned version-tag constant"
else
  bad "no pinned SYFT_VERSION constant found"
fi
if bodyhas "$SBOM_SCANNER" "^pub async fn install_syft" '/main/install.sh'; then
  bad "install_syft still fetches install.sh from the mutable main ref"
else
  ok "install_syft no longer fetches install.sh from /main/"
fi
if bodyhasre "$SBOM_SCANNER" "^pub async fn install_syft" 'syft/\{SYFT_VERSION\}/install\.sh'; then
  ok "install_syft fetches install.sh from the pinned SYFT_VERSION tag ref"
else
  bad "install_syft's install.sh fetch is not pinned to SYFT_VERSION's ref"
fi
if bodyhasre "$SBOM_SCANNER" "^pub async fn install_syft" '\-b \{SYFT_DIR\} \{SYFT_VERSION\}'; then
  ok "install_syft passes SYFT_VERSION as install.sh's TAG argument"
else
  bad "install_syft no longer pins the installed release via install.sh's TAG arg"
fi

echo
echo "── 4. S3 (syft half): kill_on_drop on both spawns in sbom_scanner.rs ──"
if bodyhas "$SBOM_SCANNER" "^pub async fn install_syft" "kill_on_drop(true)"; then
  ok "install_syft sets kill_on_drop(true) on its spawn"
else
  bad "install_syft is missing kill_on_drop(true)"
fi
if bodyhas "$SBOM_SCANNER" "^pub async fn generate_sbom" "kill_on_drop(true)"; then
  ok "generate_sbom sets kill_on_drop(true) on the syft invocation"
else
  bad "generate_sbom is missing kill_on_drop(true) — a timed-out SBOM run can orphan"
fi

echo
echo "── 5. S4: cms.rs — a shared credential-file permission helper, wired into the 4 CMS installers that hold DB creds ──"
if bodyhasre "$CMS" "^async fn secure_credential_file" 'chmod.*640'; then
  ok "secure_credential_file chmods its target to 640"
else
  bad "secure_credential_file no longer chmods to 640 — the helper's own body changed shape"
fi
for pair in "install_laravel:env_file" "install_codeigniter:env_file"; do
  fn="${pair%%:*}"; var="${pair##*:}"
  if bodyhasre "$CMS" "^pub async fn $fn" "secure_credential_file\(&$var\)"; then
    ok "$fn now secures its .env file's permissions"
  else
    bad "$fn no longer calls secure_credential_file on its .env file"
  fi
done
if bodyhas "$CMS" "^pub async fn install_drupal" "secure_credential_file" && \
   bodyhas "$CMS" "^pub async fn install_drupal" "settings.php"; then
  ok "install_drupal secures drush's generated settings.php"
else
  bad "install_drupal no longer secures settings.php's permissions"
fi
if bodyhas "$CMS" "^pub async fn install_joomla" "secure_credential_file" && \
   bodyhas "$CMS" "^pub async fn install_joomla" "configuration.php"; then
  ok "install_joomla secures the generated configuration.php"
else
  bad "install_joomla no longer secures configuration.php's permissions"
fi
# Positive control: Symfony's install() takes no db_pass at all (confirmed by
# its own signature) — it must NOT be wired, or a future blanket copy-paste
# would be chmod'ing a file that was never proven to hold credentials here.
if bodyhas "$CMS" "^pub async fn install_symfony" "secure_credential_file"; then
  bad "install_symfony now calls secure_credential_file — re-check whether Symfony's .env gained DB creds in this function (if so, this control is stale, not a regression)"
else
  ok "install_symfony correctly left unwired (control: its own signature takes no db_pass)"
fi

echo
echo "── 6. S4: wordpress.rs::install — tightens wp-config.php after wp-cli writes it ──"
if bodyhas "$WORDPRESS" "^pub async fn install\(" "chmod" && \
   bodyhas "$WORDPRESS" "^pub async fn install\(" "640" && \
   bodyhas "$WORDPRESS" "^pub async fn install\(" "wp-config.php"; then
  ok "install() chmods wp-config.php to 640"
else
  bad "install() no longer tightens wp-config.php's permissions"
fi
if bodyhas "$WORDPRESS" "^pub async fn install\(" "www-data:www-data"; then
  ok "install() still chowns the site to www-data:www-data (positive control — the chown wasn't replaced, only supplemented)"
else
  bad "install() no longer chowns the site at all — unrelated regression"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
