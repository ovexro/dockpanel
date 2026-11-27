#!/usr/bin/env bash
#
# Regression pin: EL10 (AlmaLinux 10, Rocky 10, CentOS Stream 10) and Fedora 41+.
#
# Two things those releases changed underneath the agent:
#
#   REDIS IS GONE. The `redis` package was replaced by its wire-compatible fork
#   `valkey` (valkey.service, valkey-server, valkey-cli). The agent mapped
#   redis-server to `redis` for the package AND the unit and called `redis-cli`
#   by name, so the Redis installer, the Rspamd installer (which installs Redis
#   too), the per-site object cache and every Redis status check failed there.
#
#   PHP HAS NO MODULE STREAMS. php-fpm ships non-modular in AppStream, so
#   php_streams() is empty, and install_version() fell through to the apt
#   branch and answered "Unsupported distro for PHP install: ID='almalinux'".
#
# EL9 and Fedora 39 keep `redis` and module streams; both paths must stay.
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
ROUTES=panel/agent/src/routes
for f in "$PKG" "$PHP" "$SI"; do
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
has()  { grep -qE -- "$2" <<<"$1"; }
nonempty() { [ "$(grep -c . <<<"$1")" -ge "$2" ]; }

# Lists every `safe_command("redis-cli")` in the given tree (code only).
scan_redis_cli() {
  local f
  while IFS= read -r f; do
    code_of "$f" | grep -nF 'safe_command("redis-cli")' | sed "s#^#$f:#"
  done < <(find "$1" -name '*.rs' | sort)
}

# Prints OK when the RPM guard comes before the first apt-cache use in a body.
rpm_before_apt() {
  local rpm apt
  rpm=$(grep -nE 'PkgMgr::Rpm' <<<"$1" | head -1 | cut -d: -f1)
  apt=$(grep -nF '"apt-cache' <<<"$1" | head -1 | cut -d: -f1)
  if [ -z "$apt" ]; then apt=$(grep -nF 'apt-cache' <<<"$1" | head -1 | cut -d: -f1); fi
  [ -n "$rpm" ] && [ -n "$apt" ] && [ "$rpm" -lt "$apt" ] && echo OK
}

echo "§1 Redis is Valkey on EL10 and Fedora 41+, and Redis everywhere else"

B=$(fnbody "$PKG" rpm_name_for)
if nonempty "$B" 3 && has "$B" '"redis-server" && use_valkey' && has "$B" '"valkey"'; then
  ok "the package map sends redis-server to valkey when the box uses Valkey"
else
  bad "rpm_name_for no longer maps redis-server to valkey — dnf would ask for a package EL10 does not have"
fi

B=$(fnbody "$PKG" rpm_service_name_for)
if nonempty "$B" 3 && has "$B" '"redis-server" && use_valkey' && has "$B" '"valkey"'; then
  ok "the unit map sends redis-server to valkey.service when the box uses Valkey"
else
  bad "rpm_service_name_for no longer maps to valkey — systemctl would enable a unit that does not exist"
fi

B=$(fnbody "$PKG" os_defaults_to_valkey)
if has "$B" '"fedora" => major >= 41' && has "$B" '"almalinux" \| "rocky" \| "centos".* => major >= 10'; then
  ok "the os-release fallback picks Valkey from Fedora 41 and EL10 on, not before"
else
  bad "os_defaults_to_valkey changed its cut-over — EL9 and Fedora 39 would lose redis, or EL10 would keep asking for it"
fi

B=$(fnbody "$PKG" rpm_uses_valkey)
if has "$B" '"rpm", &\["-q", "valkey"\]' && has "$B" 'os_defaults_to_valkey'; then
  ok "detection asks what is installed first and falls back to os-release, not to the dnf query alone"
else
  bad "rpm_uses_valkey must check the installed package and keep the os-release fallback"
fi

B=$(fnbody "$PKG" is_installed)
if has "$B" '"redis-server"' && has "$B" '"valkey"'; then
  ok "is_installed(redis-server) counts an installed valkey"
else
  bad "is_installed(redis-server) ignores valkey — the Services page reports Redis absent on EL10"
fi

B=$(fnbody "$PKG" transact)
if has "$B" 'Action::Remove' && has "$B" '"valkey"'; then
  ok "removing redis-server removes valkey when that is what is installed"
else
  bad "transact() no longer resolves redis-server to valkey on removal"
fi

HITS=$(scan_redis_cli "$ROUTES")
if [ -z "$HITS" ]; then
  ok "no route runs redis-cli by name"
else
  bad "these routes run redis-cli by name, which EL10 does not ship: $(tr '\n' ' ' <<<"$HITS")"
fi
USES=$(code_of "$ROUTES/nginx.rs"; code_of "$SI")
N=$(grep -cF 'redis_cli_bin()' <<<"$USES" || true)
if [ "$N" -ge 3 ]; then
  ok "$N call sites resolve the CLI through pkg::redis_cli_bin()"
else
  bad "only $N call sites use pkg::redis_cli_bin() — expected the cache enable/flush and the installer"
fi

B=$(fnbody "$SI" install_status)
if has "$B" 'redis_server_installed\(\)' && has "$B" 'service_name\("redis-server"\)'; then
  ok "install_status asks for Redis through the family-aware helpers"
else
  bad "install_status checks redis-server by its Debian names again"
fi

echo
echo "§2 PHP on a family with no module streams does not fall into the apt branch"

B=$(fnbody "$PHP" install_version)
if nonempty "$B" 20 && [ "$(rpm_before_apt "$B")" = OK ]; then
  ok "install_version branches on the RPM family before any apt-cache use"
else
  bad "install_version reaches apt machinery before the RPM guard — EL10 answers 'Unsupported distro'"
fi
if has "$B" 'available_rpm_php_version\(\)'; then
  ok "install_version asks the non-modular repository which PHP it offers"
else
  bad "install_version no longer consults available_rpm_php_version() — EL10 has no streams to look at"
fi

B=$(fnbody "$SI" install_php)
if has "$B" 'available_rpm_php_version\(\)'; then
  ok "install_php picks the version a non-modular RPM box actually offers"
else
  bad "install_php probes apt-cache on an RPM box with no streams and guesses a version"
fi

B=$(fnbody "$PKG" available_rpm_php_version)
if has "$B" 'DNF_QUERY_CACHEDIR' && has "$B" '"repoquery"'; then
  ok "the version probe uses the writable query cache (it runs inside the sandbox)"
else
  bad "available_rpm_php_version queries dnf without DNF_QUERY_CACHEDIR — Errno 30 inside the sandbox"
fi

echo
echo "§3 controls: the arms fire on the real defects"

FIX=$(mktemp -d)
trap 'rm -rf "$FIX"' EXIT
printf 'async fn redis_enable() {\n    let check = safe_command("redis-cli").args(["ping"]).output();\n}\n' > "$FIX/nginx.rs"
if [ -n "$(scan_redis_cli "$FIX")" ]; then
  ok "a planted redis-cli call is reported (negative control fired)"
else
  bad "the redis-cli scan missed a planted call — it cannot fail"
fi

PLANT=$'let ok = safe_command("bash").args(["-c", "apt-cache show php8.3-fpm"]);\nif manager().await == PkgMgr::Rpm {\n}'
if [ "$(rpm_before_apt "$PLANT")" = OK ]; then
  bad "the order check accepted an RPM guard placed after apt-cache — it cannot fail"
else
  ok "an RPM guard placed after apt-cache is rejected (negative control fired)"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
