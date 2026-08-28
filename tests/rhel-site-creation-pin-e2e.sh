#!/usr/bin/env bash
#
# Regression pin: no site of ANY runtime could ever be created on the RHEL
# family, and PHP sites specifically failed a layer earlier than that too.
#
# TWO independent, stacked defects, found in the same session by trying to
# actually create a PHP site on a real Rocky 9 box.
#
# Defect 1 — the PHP-FPM socket. A carried tech-debt note (many sessions old)
# claimed `settle()` was a false positive on RHEL — it is not;
# `socket_exists()`/`settle()` in php.rs were already RHEL-aware. The real gap
# was one layer downstream: `put_site()` in nginx.rs validated and
# existence-checked `php_socket` as a LITERAL path
# (`/run/php/php{version}-fpm.sock`, Debian-shaped), with no fallback to
# RHEL's actual, unversioned socket (`/run/php-fpm/www.sock`). So on a real
# Rocky/Alma/CentOS box, PHP-FPM would install and report running correctly —
# and creating the very next PHP site was flatly refused with "PHP {version}
# is not installed, or its PHP-FPM service is not running", a false and
# unactionable message (reinstalling PHP cannot fix a path assumption). Fixed
# by `services::pkg::resolve_php_fpm_socket()`, the single place that knows
# both families' socket layouts, which php.rs::socket_exists() and
# nginx.rs::put_site() both now call instead of re-implementing their own
# existence check. sites.rs's hardcoded Debian-shaped string is now just a
# VERSION CARRIER on the wire, not an assertion about the real path — the
# backend cannot know the target's distro, so it was never the right layer.
#
# Defect 2 — found immediately after fixing defect 1, by hitting the NEXT
# wall live: nginx's vhost directory itself. Debian ships
# `sites-available`/`sites-enabled`; the RHEL family's nginx package has
# NEITHER — its `nginx.conf` includes only `conf.d/*.conf`. Roughly 25 call
# sites across a dozen files hardcoded the literal Debian path independently
# (`vhost_paths()` itself, plus every SSL/git-deploy/Docker-app/diagnostics/
# security-scanner/IaC-export/WAF-uninstall reader and writer of a site's
# vhost), so on RHEL EVERY write failed `ENOENT` regardless of runtime —
# static, PHP, Node, Python, proxy, all of them. Fixed by
# `services::nginx::sites_dir_for()`/`vhost_paths_for()`, mirroring the same
# `[ -d /etc/nginx/sites-enabled ]` detection `setup.sh::configure_nginx`
# already used for the PANEL's own vhost, with every other call site
# propagated to the one shared function instead of its own copy.
#
# Live-fire proof, both defects, same Rocky Linux 9.8 box, real command:
#   pre-fix (published v2.172.0):  "PHP 8.3 is not installed..." (defect 1)
#   defect-1-fixed only:           "Failed to write config: No such file
#                                    or directory" (defect 2, freshly exposed)
#   both fixed:                    site created, vhost at
#                                    /etc/nginx/conf.d/{domain}.conf,
#                                    `fastcgi_pass unix:/run/php-fpm/www.sock`,
#                                    `nginx -t` clean, curl returned
#                                    `PHP_WORKS:8.3.33` with
#                                    `X-Powered-By: PHP/8.3.33` over HTTP 200.
# Disable/enable (park/unpark) re-driven on the same site: parked file lands
# at `{domain}.conf.disabled` in the SAME conf.d directory (RHEL has no
# separate sites-available), 503 maintenance page after a reload settles,
# PHP output returns unchanged after re-enable.
#
# Pure source analysis below: no box, no network, no build — the box above
# already proved the mechanism live; this suite pins the STRUCTURE so a
# future refactor cannot silently reintroduce either literal Debian-only
# check.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

PKG_RS=panel/agent/src/services/pkg.rs
PHP_RS=panel/agent/src/routes/php.rs
NGINX_ROUTE_RS=panel/agent/src/routes/nginx.rs
NGINX_SVC_RS=panel/agent/src/services/nginx.rs
SITES_RS=panel/backend/src/routes/sites.rs
OWNERSHIP_RS=panel/agent/src/services/ownership.rs
GIT_BUILD_SVC_RS=panel/agent/src/services/git_build.rs
DOCKER_APPS_ROUTE_RS=panel/agent/src/routes/docker_apps.rs
DOCKER_APPS_SVC_RS=panel/agent/src/services/docker_apps.rs
SERVICE_INSTALLER_RS=panel/agent/src/routes/service_installer.rs
IAC_RS=panel/agent/src/routes/iac.rs
SECURITY_SCANNER_RS=panel/agent/src/services/security_scanner.rs
DIAGNOSTICS_RS=panel/agent/src/services/diagnostics.rs
MAIL_RS=panel/agent/src/routes/mail.rs
for f in "$PKG_RS" "$PHP_RS" "$NGINX_ROUTE_RS" "$NGINX_SVC_RS" "$SITES_RS" \
         "$OWNERSHIP_RS" "$GIT_BUILD_SVC_RS" "$DOCKER_APPS_ROUTE_RS" "$DOCKER_APPS_SVC_RS" \
         "$SERVICE_INSTALLER_RS" "$IAC_RS" "$SECURITY_SCANNER_RS" "$DIAGNOSTICS_RS" "$MAIL_RS"; do
  [ -f "$f" ] || { echo "MISSING SUBJECT FILE: $f"; exit 1; }
done

# Strip // line comments so an arm can't match its own explanatory prose
# (lesson #149) — the fix's comments deliberately name both families' paths.
strip() { sed 's://.*::' "$1"; }

SRC_PKG=$(strip "$PKG_RS")
SRC_PHP=$(strip "$PHP_RS")
SRC_NGINX_ROUTE=$(strip "$NGINX_ROUTE_RS")
SRC_NGINX_SVC=$(strip "$NGINX_SVC_RS")
SRC_OWNERSHIP=$(strip "$OWNERSHIP_RS")
SRC_GIT_BUILD_SVC=$(strip "$GIT_BUILD_SVC_RS")
SRC_DOCKER_APPS_ROUTE=$(strip "$DOCKER_APPS_ROUTE_RS")
SRC_DOCKER_APPS_SVC=$(strip "$DOCKER_APPS_SVC_RS")
SRC_SERVICE_INSTALLER=$(strip "$SERVICE_INSTALLER_RS")
SRC_IAC=$(strip "$IAC_RS")
SRC_SECURITY_SCANNER=$(strip "$SECURITY_SCANNER_RS")
SRC_DIAGNOSTICS=$(strip "$DIAGNOSTICS_RS")
SRC_MAIL=$(strip "$MAIL_RS")
SRC_SITES=$(strip "$SITES_RS")

# Here-strings, never pipelines: a `grep -q` closing the pipe SIGPIPEs the
# upstream sed, and pipefail turns that into a failed arm non-deterministically.
has()  { grep -q  -- "$2" <<< "$1"; }
hasE() { grep -qE -- "$2" <<< "$1"; }
fnbody() { awk "/$2/,/^}/" <<< "$1"; }
readable() { [ -n "$2" ] || bad "could not extract $1 — the arms reading it mean nothing"; }

echo
echo "§1 one resolver knows both families' socket layouts"

RESOLVER=$(fnbody "$SRC_PKG" 'pub async fn resolve_php_fpm_socket')
readable "resolve_php_fpm_socket" "$RESOLVER"

if has "$RESOLVER" '/run/php/php{version}-fpm.sock'; then
  ok "the resolver checks the Debian versioned path"
else
  bad "the resolver does not check the Debian path — apt boxes would regress"
fi

if has "$RESOLVER" '/run/php-fpm/www.sock'; then
  ok "the resolver checks the RHEL unversioned path"
else
  bad "the resolver does not check RHEL's socket — the whole point of the fix"
fi

if has "$RESOLVER" 'installed_php_version().await.as_deref() == Some(version)'; then
  ok "the RHEL branch confirms it's THIS version before trusting the unversioned socket"
else
  bad "no version confirmation on the RHEL branch — every version would claim the one live socket"
fi

echo
echo "§2 the two call sites share the resolver, not their own copies"

SOCKET_EXISTS=$(fnbody "$SRC_PHP" 'async fn socket_exists')
readable "socket_exists" "$SOCKET_EXISTS"
if has "$SOCKET_EXISTS" 'resolve_php_fpm_socket(version).await.is_some()'; then
  ok "php.rs::socket_exists() delegates to the shared resolver"
else
  bad "php.rs::socket_exists() re-implements its own check — the two can drift apart again"
fi

# The historical bug, pinned as an absence: put_site() must NOT independently
# stat the raw wire-format path as its verdict. It may still strip/parse the
# string to extract a version, but the EXISTENCE question has to go through
# the resolver.
PUT_SITE=$(fnbody "$SRC_NGINX_ROUTE" 'async fn put_site')
readable "put_site" "$PUT_SITE"

if hasE "$PUT_SITE" 'resolve_php_fpm_socket\(version\)\.await'; then
  ok "put_site() resolves the real socket instead of trusting the literal path"
else
  bad "put_site() does not call the resolver — RHEL site creation is unfixed"
fi

if hasE "$PUT_SITE" '!std::path::Path::new\(socket_path\)\.exists\(\)'; then
  bad "put_site() still has the old literal-existence check — the RHEL bug is back"
else
  ok "the old Debian-only literal existence check is gone from put_site()"
fi

echo
echo "§3 a resolved RHEL socket naturally skips the Debian-only per-site pool"

# The per-site FPM pool feature (memory/worker limits) is Debian-shaped
# (/etc/php/{version}/fpm/pool.d) and out of scope for this fix. It must
# degrade to the shared pool on RHEL, not error — proven by the guard that
# gates it still matching only a Debian-shaped resolved socket.
POOL_BLOCK=$(awk '/let mut per_site_socket: Option<String> = None;/,/if let Some\(sock\) = per_site_socket \{/' <<< "$SRC_NGINX_ROUTE")
readable "the per-site pool block" "$POOL_BLOCK"

if hasE "$POOL_BLOCK" 'strip_prefix\("unix:/run/php/php"\)'; then
  ok "the per-site pool block only activates on a Debian-shaped resolved socket"
else
  bad "the per-site pool block's guard changed — verify it still excludes RHEL's resolved socket"
fi

WRITE_POOL=$(fnbody "$SRC_NGINX_SVC" 'pub fn write_php_pool_config')
readable "write_php_pool_config" "$WRITE_POOL"
if has "$WRITE_POOL" 'return Ok(());'; then
  ok "write_php_pool_config no-ops rather than erroring when the Debian pool.d tree is absent"
else
  bad "write_php_pool_config no longer degrades gracefully on a missing pool.d tree"
fi

echo
echo "§4 the backend's hardcoded string is documented as a version carrier"

# sites.rs cannot know the target server's distro, so it is correct for it to
# keep sending the Debian-shaped literal — but only if that's understood as
# an input to the resolver, not an assertion of the real path. Pinned as a
# comment so a future reader doesn't "fix" this into a redundant distro branch
# on the wrong side of the wire.
carrier_comments=$(grep -c 'VERSION CARRIER\|[Vv]ersion carrier' "$SITES_RS")
if [ "$carrier_comments" -ge 4 ]; then
  ok "all $carrier_comments php_socket construction sites document the version-carrier contract"
else
  bad "only $carrier_comments of the 4 known php_socket construction sites are documented"
fi

echo
echo "§5 the version-status API stops advertising a socket that may not be real"

LIST_VERSIONS=$(fnbody "$SRC_PHP" 'async fn list_versions')
readable "list_versions" "$LIST_VERSIONS"
if has "$LIST_VERSIONS" 'resolve_php_fpm_socket(v).await'; then
  ok "the versions API reports the real resolved socket, not a Debian-only guess"
else
  bad "the versions API still hardcodes the Debian socket path in its response"
fi

echo
echo "§6 the nginx vhost directory itself — no site of ANY runtime worked on RHEL"

SITES_DIR_FOR=$(fnbody "$SRC_NGINX_SVC" 'pub fn sites_dir_for')
readable "sites_dir_for" "$SITES_DIR_FOR"
if hasE "$SITES_DIR_FOR" '"/etc/nginx/sites-enabled"' && hasE "$SITES_DIR_FOR" '"/etc/nginx/conf.d"'; then
  ok "one function knows both families' vhost directories"
else
  bad "sites_dir_for does not name both directories — the RHEL branch may be gone"
fi

# Split-for-testability, the same shape this file already used for
# vhost_target_between/vhost_target: the pure decision takes a bool so a test
# needs no nginx installed. A version that probed the filesystem directly
# would pass or fail by accident of whether the CI runner has nginx — pin
# against that regression explicitly.
if hasE "$SRC_NGINX_SVC" 'fn sites_dir\(\) -> &.static str \{' ; then
  SITES_DIR_UNCONDITIONAL=$(fnbody "$SRC_NGINX_SVC" 'pub fn sites_dir\(\)')
  if has "$SITES_DIR_UNCONDITIONAL" 'sites_dir_for('; then
    ok "the real-filesystem wrapper delegates to the pure, testable decision"
  else
    bad "sites_dir() does not delegate to sites_dir_for — the decision is no longer testable without nginx installed"
  fi
else
  bad "could not find sites_dir() — the real-filesystem entry point is gone"
fi

VHOST_PATHS_FOR=$(fnbody "$SRC_NGINX_SVC" 'pub fn vhost_paths_for')
readable "vhost_paths_for" "$VHOST_PATHS_FOR"
if has "$VHOST_PATHS_FOR" 'sites_dir_for(uses_sites_enabled)'; then
  ok "vhost_paths_for shares the same directory decision, not a second copy"
else
  bad "vhost_paths_for re-derives the directory instead of sharing sites_dir_for"
fi
if hasE "$VHOST_PATHS_FOR" 'if uses_sites_enabled \{' && has "$VHOST_PATHS_FOR" '} else {'; then
  ok "vhost_paths_for actually branches on the convention rather than returning one shape"
else
  bad "vhost_paths_for has no visible branch on uses_sites_enabled — one family always gets the other's paths"
fi

echo
echo "§7 every other reader/writer of a site's vhost shares the one decision, not its own copy"

# The class of defect: ~25 call sites across a dozen files independently
# hardcoded the Debian path. Each subject below must call through sites_dir()
# rather than spelling /etc/nginx/sites-enabled itself. mail.rs is a deliberate
# exception — it already tried both literal paths for the PANEL's own vhost
# before this fix existed, which is a different (already-correct) shape.
declare -A PROPAGATION=(
  ["ssl.rs (routes)"]="panel/agent/src/routes/ssl.rs"
  ["ownership.rs"]="$OWNERSHIP_RS"
  ["git_build.rs (services)"]="$GIT_BUILD_SVC_RS"
  ["docker_apps.rs (routes)"]="$DOCKER_APPS_ROUTE_RS"
  ["docker_apps.rs (services)"]="$DOCKER_APPS_SVC_RS"
  ["service_installer.rs"]="$SERVICE_INSTALLER_RS"
  ["iac.rs"]="$IAC_RS"
  ["security_scanner.rs"]="$SECURITY_SCANNER_RS"
  ["diagnostics.rs"]="$DIAGNOSTICS_RS"
)
for label in "${!PROPAGATION[@]}"; do
  f="${PROPAGATION[$label]}"
  src=$(strip "$f")
  if hasE "$src" '"/etc/nginx/sites-enabled/'; then
    bad "$label still hardcodes the Debian vhost directory literally"
  elif has "$src" 'sites_dir()'; then
    ok "$label reads the vhost directory through the shared decision"
  else
    bad "$label neither hardcodes nor calls sites_dir() — re-derive what changed"
  fi
done

# mail.rs is the one file that got this right BEFORE the fix existed — it
# tries both literal paths for the panel's own vhost. Pin that this is still
# true, as a control: if this ever regresses to hardcoding one path, the
# panel's own webmail-domain detection breaks on whichever family it drops.
if hasE "$SRC_MAIL" '"/etc/nginx/sites-enabled/dockpanel-panel.conf"' && hasE "$SRC_MAIL" '"/etc/nginx/conf.d/dockpanel-panel.conf"'; then
  ok "mail.rs's pre-existing dual-path check for the panel's own vhost is still intact (control)"
else
  bad "mail.rs no longer tries both families for the panel's own vhost"
fi

echo
echo "§8 the parked (disabled) directory: RHEL shares one dir, and a shared-dir scan is not double-counted"

PARKED_DIR_FOR=$(fnbody "$SRC_NGINX_SVC" 'pub fn parked_dir_for')
readable "parked_dir_for" "$PARKED_DIR_FOR"
if has "$PARKED_DIR_FOR" 'sites_dir_for(false)'; then
  ok "on the RHEL branch, parked_dir_for falls back to the SAME directory as sites_dir_for"
else
  bad "parked_dir_for's RHEL branch does not point at the same directory as sites_dir_for"
fi

REGISTRY_REFS=$(fnbody "$SRC_OWNERSHIP" 'fn registry_needle_references')
readable "registry_needle_references" "$REGISTRY_REFS"
if hasE "$REGISTRY_REFS" 'parked_dir == live_dir'; then
  ok "a shared live/parked directory short-circuits before the second scan"
else
  bad "no guard against scanning the same directory twice — RHEL would double-count every match"
fi

if has "$SRC_OWNERSHIP" 'a_single_shared_directory_is_not_scanned_twice'; then
  ok "the double-scan guard has its own mutation-reachable unit test"
else
  bad "no unit test exercises the double-scan guard directly"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
