#!/usr/bin/env bash
#
# Regression pin: parallel multi-PHP (8.1-8.5) on RPM distributions via Remi SCL.
#
# Before this change an RPM host could only run the single PHP version shipped
# as `php-fpm` by the distro (one module stream on EL9, or PHP 8.3 alone on
# EL10). Requesting another version (e.g. PHP 8.4 or 8.5 on EL10, or a second
# version beside the system stream on EL9) returned HTTP 501.
#
# Remi's SCL packages (`php81-php-fpm`..`php85-php-fpm`) install cleanly in
# parallel, each with its own systemd unit, CLI symlink and Unix socket under
# `/var/opt/remi/php{XY}/run/php-fpm/www.sock`. Several invariants are load-
# bearing and pinned here:
#   - `php_rpm_name()` must pass `php84-php-*` through untouched instead of
#     collapsing it onto system `php-fpm`.
#   - `install_version_remi()` must grant `nginx` ACL access in Remi's
#     `www.conf` (`listen.acl_users = apache` -> `apache,nginx`), otherwise
#     every Nginx request returns 502 Permission denied.
#   - `uninstall_version()` must remove only `'{prefix}-php-*'` for Remi
#     versions so removing one never wipes system PHP or sibling SCLs.
#   - `list_extensions()` must pass `DNF_QUERY_CACHEDIR` on the Remi dnf query
#     so it succeeds under `ProtectSystem=strict`.
#   - `remi_release_url()` covers supported community EL rebuilds + Fedora and
#     does not claim RHEL/Oracle Linux where base repos lack `epel-release`.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

PKG=panel/agent/src/services/pkg.rs
PHP=panel/agent/src/routes/php.rs
SI=panel/agent/src/routes/service_installer.rs
NGINX=panel/agent/src/services/nginx.rs
for f in "$PKG" "$PHP" "$SI" "$NGINX"; do
  [ -f "$f" ] || { echo "missing $f"; exit 1; }
done

# Code only: test module cut at its cfg line, // comments blanked.
code_of() {
  awk '/^#\[cfg\((.*[^A-Za-z0-9_])?test([^A-Za-z0-9_].*)?\)\]/ { exit }
       { sub(/\/\/.*$/, ""); print }' "$1"
}
# One top-level fn body (closing brace in column 0), comment-stripped.
fnbody() { code_of "$1" | awk "/^(pub )?(async )?fn $2\\(/,/^}/"; }
# Here-strings, never pipelines into grep -q (pipefail + SIGPIPE).
has()      { grep -qE -- "$2" <<<"$1"; }
nonempty() { [ "$(grep -c . <<<"$1")" -ge "$2" ]; }

has_remi_acl_patch() {
  has "$1" 'listen\\\\\.acl_users' && has "$1" 'nginx' && has "$1" '/etc/opt/remi/'
}

has_surgical_remi_uninstall() {
  has "$1" 'is_remi' && has "$1" 'dnf remove -y .\{prefix\}-php-\*'
}

echo "§1 Remi SCL naming, socket, unit, and package safety"

B=$(fnbody "$PKG" remi_php_prefix)
if nonempty "$B" 4 && has "$B" "split_once\('\.'\)" && has "$B" 'is_ascii_digit\(\)'; then
  ok "remi_php_prefix validates numeric major.minor segments before formatting"
else
  bad "remi_php_prefix does not validate numeric major.minor segments"
fi

B=$(fnbody "$PKG" php_rpm_name)
if nonempty "$B" 5 && has "$B" '!ext\.starts_with\("php-"\)' ; then
  ok "php_rpm_name passes Remi SCL package names (phpXY-php-*) through untouched"
else
  bad "php_rpm_name would collapse Remi SCL names onto system php-*"
fi

B=$(fnbody "$PKG" resolve_php_fpm_socket)
if has "$B" 'remi_php_socket_path' && has "$B" '/run/php/php' && has "$B" '/run/php-fpm/www\.sock'; then
  ok "resolve_php_fpm_socket checks Debian, Remi SCL, and system RPM sockets"
else
  bad "resolve_php_fpm_socket lost one of the three socket paths"
fi

B_UNIT=$(fnbody "$PKG" php_fpm_unit_for)
B_RELOAD=$(fnbody "$NGINX" reload_php_fpm)
if has "$B_UNIT" 'is_remi_php_installed' && has "$B_UNIT" 'remi_php_fpm_unit' && has "$B_RELOAD" 'php_fpm_unit_for'; then
  ok "php_fpm_unit_for dispatches Remi SCL units and nginx reload_php_fpm uses it"
else
  bad "php_fpm_unit_for or nginx::reload_php_fpm no longer routes Remi SCL units"
fi

B=$(fnbody "$PKG" remi_release_url)
if has "$B" '"almalinux" \| "rocky" \| "centos"' && has "$B" '"fedora"' && ! has "$B" '"rhel"|"ol"'; then
  ok "remi_release_url targets supported EL rebuilds + Fedora and excludes unsupported rhel/ol"
else
  bad "remi_release_url distro set drifted (expected almalinux/rocky/centos + fedora, no rhel/ol)"
fi

echo
echo "§2 Remi repository setup and rollback hygiene"

B=$(fnbody "$PKG" add_repo)
if has "$B" 'Repo::Remi' && has "$B" 'curl -sfI' && has "$B" 'epel-release' && has "$B" '--set-enabled crb'; then
  ok "add_repo(Repo::Remi) pre-checks URL reachability and enables EPEL + CRB on EL"
else
  bad "add_repo(Repo::Remi) is missing reachability pre-check or EPEL/CRB setup"
fi

B=$(fnbody "$PKG" repo_artifact_paths)
if has "$B" 'Repo::Remi' && has "$B" 'remi-safe\.repo' && has "$B" 'remi\.repo' && has "$B" 'remi-modular\.repo'; then
  ok "repo_artifact_paths(Repo::Remi) tracks all three Remi repo files for rollback"
else
  bad "repo_artifact_paths(Repo::Remi) is incomplete — failed setup would leave repo files behind"
fi

echo
echo "§3 Install, extensions, status, and surgical uninstall"

B=$(fnbody "$PHP" install_version)
if has "$B" 'installed_php_version\(\)' && has "$B" 'system_php\.is_none\(\)' && has "$B" 'install_version_rpm' && has "$B" 'install_version_remi'; then
  ok "install_version installs native distro PHP first when available and uses Remi SCL otherwise"
else
  bad "install_version no longer prefers native distro PHP before falling back to Remi SCL"
fi

B=$(fnbody "$PHP" install_version_remi)
if has "$B" 'Repo::Remi' && has "$B" 'remi_php_pkg_name' && has_remi_acl_patch "$B"; then
  ok "install_version_remi configures Remi, maps SCL extensions, and patches nginx socket ACL"
else
  bad "install_version_remi is missing Repo::Remi, extension mapping, or nginx ACL patch (502 risk)"
fi

B=$(fnbody "$PHP" list_extensions)
if has "$B" 'remi_php_cli_bin' && has "$B" 'is_remi' && has "$B" 'DNF_QUERY_CACHEDIR'; then
  ok "list_extensions queries Remi CLI binary and uses DNF_QUERY_CACHEDIR inside the sandbox"
else
  bad "list_extensions lost Remi CLI or sandboxed DNF_QUERY_CACHEDIR handling"
fi

B=$(fnbody "$PHP" install_extension)
if has "$B" 'is_remi_php_installed' && has "$B" 'remi_php_pkg_name' && has "$B" 'php_fpm_unit_for'; then
  ok "install_extension maps Remi extension names and restarts the right FPM unit"
else
  bad "install_extension no longer supports Remi SCL packages or unit names"
fi

B=$(fnbody "$PHP" uninstall_version)
if has_surgical_remi_uninstall "$B"; then
  ok "uninstall_version removes only '{prefix}-php-*' for Remi versions, preserving sibling installs"
else
  bad "uninstall_version does not scope Remi removals to '{prefix}-php-*'"
fi

B_ST=$(fnbody "$SI" install_status)
B_VER=$(fnbody "$SI" detect_php_version)
if has "$B_ST" 'is_remi_php_installed' && has "$B_ST" 'remi_php_fpm_unit' && has "$B_VER" 'is_remi_php_installed'; then
  ok "service_installer install_status and detect_php_version recognize Remi-only RPM hosts"
else
  bad "service_installer ignores Remi SCL installs on the Services status page"
fi

echo
echo "§4 controls: the arms fire on the real defects"

PLANT_UNINSTALL=$'let purge_cmd = if rpm {\n    "dnf remove -y \x27php-*\x27 2>&1".to_string()\n};'
if has_surgical_remi_uninstall "$PLANT_UNINSTALL"; then
  bad "the uninstall check accepted an unscoped dnf remove 'php-*' — it cannot fail"
else
  ok "an unscoped dnf remove 'php-*' is rejected (negative control fired)"
fi

PLANT_ACL=$'async fn install_version_remi(version: &str) {\n    pkg::install(&[fpm_pkg, cli_pkg]).await;\n    systemctl_restart(&unit);\n}'
if has_remi_acl_patch "$PLANT_ACL"; then
  bad "the ACL check accepted install_version_remi without the nginx ACL patch — it cannot fail"
else
  ok "install_version_remi without the nginx ACL patch is rejected (negative control fired)"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
