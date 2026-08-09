#!/usr/bin/env bash
# Regression pins for s264 — the RPM-family install path.
#
# README, docs/getting-started.md, docs/guides/multi-server.md and the marketing
# site all promised CentOS 9+, Rocky 9+, Fedora 39+ and Amazon Linux 2023. Nobody
# had ever run the installer on any of them. Driving all four on real boxes found
# that ALL FOUR failed, at three different places:
#
#   Rocky 9      — download.docker.com/linux/rocky/9 exists but publishes no
#                  docker-ce, so `dnf install docker-ce` died at step 3 of 15.
#   AlmaLinux 9  — get.docker.com refuses it outright ("Unsupported distribution
#                  'almalinux'"), though detect_os prints "Detected: AlmaLinux".
#   CentOS 9     — the sed that strips nginx's default server block ended its
#                  range at the first `}`, which is a nested location's brace, so
#                  it corrupted nginx.conf and nginx -t failed.
#   Fedora 43    — the agent unit listed /etc/apt in ReadWritePaths. On an RPM
#                  box that path does not exist, systemd refuses to build the
#                  mount namespace, and the agent could not start AT ALL.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

UNIT=panel/agent/dockpanel-agent.service
SETUP=scripts/setup.sh

echo "── 1. No sandbox path can make the agent unstartable on a distro that lacks it ──"

# THE DURABLE PIN. /etc/apt was not a typo — it was an allow-list entry nobody
# re-checked against a non-Debian box, and two hand-maintained mirrors of this
# same list (in setup.sh and update.sh, both commented "pre-create everything the
# canonical unit lists") had missed it for as long as it existed. So rather than
# pinning that one path, require every entry to be EITHER optional-by-prefix or
# demonstrably created before the unit starts. A future entry cannot repeat this.
RWP=$(grep -m1 '^ReadWritePaths=' "$UNIT" | sed 's/^ReadWritePaths=//')

if [ -z "$RWP" ]; then
  bad "$UNIT has no ReadWritePaths= line — this check cannot see the sandbox at all"
else
  # Paths present on any Linux box before DockPanel touches it, or created by
  # the package manager as a dependency of a step that must already have run.
  ALWAYS='/var/www /var/log /opt /etc/systemd/system /etc/nginx'
  unguarded=""
  for p in $RWP; do
    case "$p" in -*) continue ;; esac            # optional: systemd skips if absent
    case " $ALWAYS " in *" $p "*) continue ;; esac
    # Created up-front by the installer? Match a real mkdir, not a passing mention.
    if grep -qE "mkdir -p( -m [0-7]+)? [^#]*(^| )${p}( |$)" "$SETUP"; then continue; fi
    case "$p" in
      /etc/dockpanel|/var/run/dockpanel|/var/backups/dockpanel|/var/lib/dockpanel) continue ;;
    esac
    unguarded="$unguarded $p"
  done

  if [ -n "$unguarded" ]; then
    bad "ReadWritePaths lists${unguarded} with no '-' prefix and no mkdir in $SETUP — systemd fails the whole namespace mount if any one is missing, and the agent will not start"
  else
    ok "every ReadWritePaths entry is either optional ('-' prefix) or created before the unit starts"
  fi

  # The specific entry that did it, kept as a named regression.
  if grep -qE '^ReadWritePaths=.* /etc/apt( |$)' "$UNIT"; then
    bad "/etc/apt is back in ReadWritePaths unprefixed — that is the exact entry that made the agent unstartable on every RPM box"
  else
    ok "/etc/apt is not listed unprefixed"
  fi
fi

echo
echo "── 2. The nginx default-server strip counts braces ──"

# The sed range `/server {/,/^[[:space:]]*}/` looks right and is wrong: it stops
# at the first line that is only a closing brace, which inside a server block
# belongs to a nested location. It commented the opening of the block and left
# the rest at http level. Reproduced in a rockylinux:9 container against the
# stock config: `"location" directive is not allowed here in nginx.conf:52`.
if grep -qE "sed -i '/\^?\[\[:space:\]\]\*server \{/,/" "$SETUP"; then
  bad "the default-server strip is a sed range again — it ends at the first nested closing brace and corrupts nginx.conf"
else
  ok "no sed-range default-server strip in $SETUP"
fi

if grep -q 'gsub(/\\{/, "{") - gsub(/\\}/, "}")' "$SETUP"; then
  ok "the strip counts braces to find the block's real end"
else
  bad "the brace-counting default-server strip is gone from $SETUP — whatever replaced it must find the block end, not the first '}'"
fi

echo
echo "── 3. RHEL rebuilds do not depend on a repo upstream leaves empty ──"

if grep -q 'docker_repo_rhel_clone' "$SETUP"; then
  ok "an explicit Docker repo is written for the RHEL rebuilds"
else
  bad "docker_repo_rhel_clone is gone — Rocky/Alma fall back to get.docker.com, whose rocky path serves no docker-ce and whose distro list has no almalinux"
fi

if grep -q 'download.docker.com/linux/centos/\$releasever' "$SETUP"; then
  ok "that repo points at the centos path, which upstream actually fills"
else
  bad "the RHEL-rebuild Docker repo no longer points at the centos path — linux/rocky/ has metadata but no docker-ce packages"
fi

# rocky and almalinux must both reach that branch. almalinux especially: detect_os
# greets it by name, so a user has every reason to expect the install to work.
if grep -qE '^ *rocky\|almalinux\|centos\|rhel' "$SETUP"; then
  ok "rocky and almalinux are routed to the explicit repo"
else
  bad "the case that routes rocky/almalinux to the explicit Docker repo has changed shape — check both still reach it"
fi

echo
echo "── 4. The box is configured for the firewall it is actually running ──"

# s265: install succeeded on all four RPM families and the panel was still
# unreachable, because setup.sh installed UFW next to the firewalld the distro
# already had running and then opened 80/443 in UFW only. Let's Encrypt could
# not fetch the ACME challenge, so there was no certificate either — while the
# installer printed "installed successfully" and an https:// URL.

# The detector must exist AND be called from main() — a definition nothing
# invokes leaves FW_MGR at its "none" default, which silently disables every
# fw_allow below. (The first version of this pin only checked the definition
# and passed happily when the call site was removed.)
if grep -qE '^detect_firewall\(\) \{' "$SETUP" && \
   grep -qE '^[[:space:]]+detect_firewall$' "$SETUP"; then
  ok "setup.sh detects the enforcing firewall (FW_MGR) and calls the detector from main()"
else
  bad "detect_firewall is missing or never called in $SETUP — FW_MGR stays 'none', fw_allow becomes a no-op, and the installer is back to assuming UFW"
fi

# THE DURABLE PIN for this class, same shape as §1: rather than pinning one
# call site, require every `pkg_install ufw` to sit on the branch taken only
# when no firewall is running. Checked positionally, because a textual search
# for "is it inside the case" passes for a `none)` branch that no longer exists.
ufw_installs=$(grep -cE '^[[:space:]]*(if run .*)?pkg_install ufw|pkg_install ufw' "$SETUP" || true)
guarded_installs=$(awk '
  /^[[:space:]]*case "\$FW_MGR" in/ { in_case=1 }
  in_case && /^[[:space:]]*none\)/  { in_none=1 }
  in_none && /pkg_install ufw/      { n++ }
  in_none && /^[[:space:]]*;;/      { in_none=0 }
  in_case && /^[[:space:]]*esac/    { in_case=0 }
  END { print n+0 }
' "$SETUP")
if [ "$ufw_installs" -gt 0 ] && [ "$guarded_installs" -eq "$ufw_installs" ]; then
  ok "every 'pkg_install ufw' sits on the FW_MGR=none branch ($guarded_installs/$ufw_installs)"
elif [ "$ufw_installs" -eq 0 ]; then
  ok "setup.sh never installs UFW"
else
  bad "$ufw_installs 'pkg_install ufw' in $SETUP but only $guarded_installs on the FW_MGR=none branch — installing UFW next to a running firewalld is what left the panel unreachable on every RHEL-family box"
fi

if grep -q 'firewall-cmd' "$SETUP"; then
  ok "setup.sh knows how to open a port in firewalld"
else
  bad "$SETUP never mentions firewall-cmd — ports opened only in UFW are dropped by firewalld on Rocky/Alma/CentOS/Fedora"
fi

# The agent had the same defect one layer in: open_mail_ports() shelled out to
# ufw, discarded every result, and logged success unconditionally.
#
# Two kinds of ufw call are legitimate and must stay allowed: the ufw installer
# itself (`install_ufw`/`uninstall_ufw`, which now refuse on non-apt boxes) and
# ufw-specific rule CRUD. What must NOT come back is code that *opens a port*
# or *reports firewall state* through ufw alone — those are the ones that were
# wrong on every RHEL-family box. Anything matching below is that class.
# Opening a port through ufw alone is the defect. (Reading ufw's own status is
# fine where it sits behind the dispatch — see the next assertion.)
offenders=$(grep -rn 'safe_command("ufw")' panel/agent/src --include=*.rs \
  | grep -vE 'services/firewall\.rs' \
  | grep -vE 'routes/service_installer\.rs' \
  | grep -E '"allow"' || true)
if [ -n "$offenders" ]; then
  bad "code opens a port through ufw directly:
$offenders
    → use services::firewall::allow_tcp, which dispatches on the running firewall AND returns whether it worked"
else
  ok "port-opening goes through services/firewall.rs on every path"
fi

# Firewall STATUS must branch on the detected firewall rather than assuming ufw.
if grep -q 'firewalld_status' panel/agent/src/services/security.rs && \
   grep -q 'firewall::detect' panel/agent/src/services/security.rs; then
  ok "the Security page dispatches on the running firewall instead of calling a firewalld box unfirewalled"
else
  bad "security.rs no longer dispatches on the detected firewall — on the RHEL family the Security overview reports 'no firewall' for a box that is firewalled"
fi

if grep -q 'firewall::detect' panel/agent/src/services/diagnostics.rs; then
  ok "diagnostics raises 'no firewall' from the real firewall state, not from ufw's absence"
else
  bad "diagnostics.rs is back to asking ufw — it will warn 'Firewall (ufw) is not active' on every firewalld box and name a tool the operator does not have"
fi

echo
echo "── 5. Package queries work on both package databases ──"

# is_installed() ran `dpkg -l`. There is no dpkg on an RPM box, so it answered
# false for EVERY package: the Services page reported PHP and Fail2Ban as not
# installed while both were installed and running. There were four separate
# hand-rolled copies of it, which is how it stayed wrong in all of them.
if grep -rq 'safe_command("dpkg")' panel/agent/src --include=*.rs; then
  bad "an agent file calls dpkg directly — there is no dpkg on the RHEL family, so that query answers false for every package. Use services::pkg::is_installed"
else
  ok "no direct dpkg calls in the agent — package presence goes through services::pkg"
fi

if grep -q 'PkgMgr::Rpm' panel/agent/src/services/pkg.rs 2>/dev/null; then
  ok "services::pkg dispatches on the box's real package database"
else
  bad "services::pkg no longer handles rpm — every package query is Debian-only again"
fi

echo
echo "── 6. SELinux is accounted for, not discovered by the operator ──"

# With SELinux Enforcing (the RHEL-family default) nginx may not open a socket
# to the API, so every request answered 502 — including from the box itself.
# The denial is dontaudit'ed: nothing in the journal, nothing in ausearch.
if grep -q 'httpd_can_network_connect' "$SETUP"; then
  ok "setup.sh sets httpd_can_network_connect, without which the panel answers 502 on Enforcing systems"
else
  bad "$SETUP no longer sets httpd_can_network_connect — nginx cannot reach the API under Enforcing SELinux and every request 502s with no log line to explain it"
fi

# Existing broken boxes cannot be fixed from the panel, because the panel is
# what is unreachable. update.sh is the only path in.
if grep -q 'httpd_can_network_connect' scripts/update.sh && \
   grep -q 'firewall-cmd' scripts/update.sh; then
  ok "update.sh heals both defects on installs that already exist"
else
  bad "update.sh no longer repairs the firewall/SELinux state — boxes installed before v2.38.0 stay unreachable, and they cannot be fixed from a panel they cannot reach"
fi

echo
echo "── 7. The refusals that remain are TRUE, and the one that must remain does ──"

# s266 gave most optional-service installers a real dnf path, so the blanket
# apt-only refusal is gone. A refusal that has stopped being true is worse than
# no refusal, so the rename must be COMPLETE — a surviving call would be a
# handler still claiming a limitation the code below it no longer has.
if grep -rq 'apt_only_reason' panel/agent/src 2>/dev/null; then
  bad "apt_only_reason still has callers — s266 replaced it, so any survivor is a handler refusing for a reason that is no longer true"
else
  ok "the blanket apt-only refusal is gone everywhere, not just where it was convenient"
fi

# Extract a top-level Rust fn body: from its signature to the closing brace in
# column 0. Used below so assertions are about CALL SITES rather than about a
# symbol existing somewhere in the file — s265 shipped two pins that checked a
# function was *defined* and would have sat green through the exact regression
# they named.
fn_body() { awk -v pat="fn $2(" 'index($0, pat) { inside=1 } inside { print; if ($0 == "}") exit }' "$1"; }

SI=panel/agent/src/routes/service_installer.rs
PKG=panel/agent/src/services/pkg.rs
PHP=panel/agent/src/routes/php.rs

# THE ONE REFUSAL THAT MUST SURVIVE. UFW is installable from EPEL, so "the
# package exists" is true and beside the point: the RHEL family boots with
# firewalld enforcing, and s265's outage was setup.sh installing ufw beside it
# and opening 80/443 in the filter nobody consults — panel unreachable, ACME
# challenge unfetchable, no certificate. Giving this button a dnf path would
# make that outage reachable by one click.
UFW_BODY=$(fn_body "$SI" install_ufw)
if printf '%s' "$UFW_BODY" | grep -c 'ufw_refusal_reason' >/dev/null; then
  ok "install_ufw still asks whether UFW is the right firewall for this box"
else
  bad "install_ufw no longer calls ufw_refusal_reason — on the RHEL family this reinstates the s265 two-firewalls outage, with the panel unreachable and no certificate"
fi

# Positional: the refusal must come BEFORE the install, or it refuses nothing.
refuse_ln=$(printf '%s\n' "$UFW_BODY" | grep -n 'ufw_refusal_reason' | head -1 | cut -d: -f1)
inst_ln=$(printf '%s\n'  "$UFW_BODY" | grep -n 'pkg::install'        | head -1 | cut -d: -f1)
if [ -n "$refuse_ln" ] && [ -n "$inst_ln" ] && [ "$refuse_ln" -lt "$inst_ln" ]; then
  ok "install_ufw refuses before it installs, not after"
elif [ -z "$inst_ln" ]; then
  bad "install_ufw no longer installs anything — this pin can no longer see the ordering it exists to protect"
else
  bad "install_ufw installs UFW before deciding whether it should — the guard runs too late to prevent anything"
fi

# The refusal must be driven by what is RUNNING, not by the package database.
if fn_body "$PKG" ufw_refusal_reason | grep -c 'firewall::detect' >/dev/null; then
  ok "the UFW refusal is decided by the firewall actually running, not by the distro name"
else
  bad "ufw_refusal_reason no longer consults firewall::detect — it is guessing from the package manager instead of from what holds the rules"
fi

echo
echo "── 8. Package operations dispatch on the real manager ──"

# Every optional-service installer must go through the abstraction. Counting
# guarded-vs-total positionally means adding a NEW installer that shells apt
# directly fails this, rather than the pin passing because the others are fine.
INSTALLERS='install_php install_certbot install_fail2ban install_powerdns install_redis install_nodejs install_waf install_cloudflared'
total=0 routed=0 unrouted=''
for f in $INSTALLERS; do
  total=$((total+1))
  body=$(fn_body "$SI" "$f")
  if printf '%s' "$body" | grep -cE 'pkg::(install|install_available|add_repo)' >/dev/null; then
    routed=$((routed+1))
  else
    unrouted="$unrouted $f"
  fi
done
if [ "$routed" -eq "$total" ]; then
  ok "all $total optional-service installers install through services::pkg ($routed/$total)"
else
  bad "these installers bypass services::pkg and will fail on any non-apt box:$unrouted ($routed/$total routed)"
fi

# THE END-OF-LIFE PHP PIN. `dnf install php-fpm` with no stream enabled resolves
# to the non-modular base package — PHP 8.0.30 on Rocky 9, older than every
# stream the box offers and end-of-life since 2023 — while the Services page
# reports PHP installed and running. The stream must be selected FIRST.
PHP_BODY=$(fn_body "$SI" install_php)
stream_ln=$(printf '%s\n' "$PHP_BODY" | grep -n 'enable_php_stream' | head -1 | cut -d: -f1)
pinst_ln=$(printf '%s\n'  "$PHP_BODY" | grep -n 'pkg::install'      | head -1 | cut -d: -f1)
if [ -n "$stream_ln" ] && [ -n "$pinst_ln" ] && [ "$stream_ln" -lt "$pinst_ln" ]; then
  ok "install_php selects the PHP module stream before installing, not after"
else
  bad "install_php installs PHP without first enabling a module stream — on Rocky 9 that silently installs end-of-life PHP 8.0 while reporting success"
fi

# A package name and a systemd unit name are different strings that merely
# coincide on Debian. Translating one and not the other installs the right
# package and then enables a unit that is not there.
REDIS_BODY=$(fn_body "$SI" install_redis)
if printf '%s' "$REDIS_BODY" | grep -c 'service_name' >/dev/null && \
   ! printf '%s' "$REDIS_BODY" | grep -cE '"(enable|start)", *"redis-server"' >/dev/null; then
  ok "install_redis enables the translated unit name, not the hardcoded Debian one"
else
  bad "install_redis enables a hardcoded redis-server unit — that unit does not exist on the RHEL family, so Redis installs and never starts"
fi

# php.rs was the surface s265's refusal layer never reached: no family guard at
# all, so every handler failed with a raw "Failed to find executable apt-get".
if fn_body "$PHP" install_version | grep -c 'php_streams' >/dev/null; then
  ok "php.rs install_version branches on the package family before reaching apt machinery"
else
  bad "php.rs install_version has no family guard — on the RHEL family it falls through to apt-cache/ppa:ondrej and fails with 'Failed to find executable apt-get'"
fi

# The PHP page must not report every offered version as installed just because
# the RPM family collapses them onto one package.
if grep -q 'installed_php_version' "$PHP"; then
  ok "php.rs distinguishes PHP versions on a family that ships only one package"
else
  bad "php.rs asks the collapsed package query per version — on the RHEL family that reports ALL offered versions as installed"
fi

# A missing file that SKIPS is worse than one that fails: the PowerDNS schema
# path is distro-specific, and both old branches were [ -f ]-guarded, so a miss
# left an empty database behind a successful install.
if fn_body "$SI" install_powerdns | grep -c 'schema not found' >/dev/null; then
  ok "the PowerDNS SQLite schema load fails loudly when no known schema path matches"
else
  bad "the PowerDNS schema load can silently skip — the install reports success, pdns starts, and the DNS server answers nothing"
fi

# Name-map completeness for the entries that were missing and would fail
# SILENTLY (the extension loop tolerates absent packages by design, so an
# unmapped name is skipped without erroring).
#
# This asserts the ARROW, inside the mapping functions only. The first version
# of this check grepped the whole file for the package name and PASSED the
# mutation test — the string still appeared in a doc comment and in the unit
# tests after the match arm was deleted. A pin that matches its own prose is
# the source-pin trap, and it would have sat green through the regression it
# names.
# Flattened to one line: a multi-name arm puts its target on the NEXT line
# ("a" | "b" | "c" => {\n "target"\n }), which a line-oriented match cannot see.
MAPS=$( { fn_body "$PKG" rpm_name; fn_body "$PKG" php_rpm_name; } | grep -v '^\s*//' | tr '\n' ' ')
map_missing=''
while IFS='|' read -r from to; do
  [ -z "$from" ] && continue
  printf '%s' "$MAPS" | grep -cE "\"$from\"[^=]*=>[^,]*\"$to\"" >/dev/null || map_missing="$map_missing $from"
done <<'ARROWS'
pdns-backend-pgsql|pdns-backend-postgresql
pdns-backend-sqlite3|pdns-backend-sqlite
libnginx-mod-http-modsecurity|nginx-mod-modsecurity
mysql|php-mysqlnd
zip|php-pecl-zip
ARROWS
if [ -z "$map_missing" ]; then
  ok "the RPM name map still translates the packages whose Debian spellings match nothing there"
else
  bad "these names no longer map, and an unmapped name is SKIPPED rather than erroring, so the capability goes missing in silence:$map_missing"
fi

# NodeSource and Cloudflare both publish per-family repos. The agent used to
# fetch the deb script on every distro while setup.sh already branched.
ADDREPO=$(fn_body "$PKG" add_repo)
if printf '%s' "$ADDREPO" | grep -c 'rpm.nodesource.com' >/dev/null && \
   printf '%s' "$ADDREPO" | grep -c 'deb.nodesource.com' >/dev/null && \
   ! grep -q 'deb.nodesource.com' "$SI"; then
  ok "the NodeSource repo is chosen per family, and no installer hardcodes the deb one"
else
  bad "the NodeSource repo choice is not family-branched — an RPM box gets the Debian setup script, which is the half-migration s265 deliberately left alone"
fi

echo "── 6. The privileged escape hatch must never pass file descriptors again (s267) ──"

# THE WALL. `systemd-run --pipe` passes stdin/stdout/stderr over D-Bus. On the
# RHEL family the bus is dbus-broker (system_dbusd_t), and SELinux's
# file_receive hook forbids it to receive a writable pipe labelled
# unconfined_service_t — the label every systemd service's pipes carry. The
# broker drops the connection and systemd-run reports "Connection reset by
# peer", with the denial dontaudit'ed so nothing is logged. Every package
# operation on every RHEL box failed this way, for as long as the sandbox had
# existed.
#
# These assertions read CODE ONLY. safe_cmd.rs discusses `--pipe` at length in
# its doc comments precisely because it must never use it again, so a grep over
# the raw file would match the explanation and sit green through the regression
# it names — the trap that let one of s266's own pins pass its mutation test.
SAFE=panel/agent/src/safe_cmd.rs
# Production code only: comments are stripped, and everything from #[cfg(test)]
# onward is dropped. The unit tests must SPELL the forbidden flag in order to
# assert its absence, and a scanner cannot tell a guard from a use — so the
# guard would convict itself. (Rust's own tests cover that region.)
code_only() { sed '/#\[cfg(test)\]/q' "$1" | grep -vE '^[[:space:]]*(///|//!|//|\*)'; }

if code_only "$SAFE" | grep -c -- '--pipe' >/dev/null; then
  bad "$SAFE passes --pipe to systemd-run again — that is THE WALL, and it makes every package operation fail on every SELinux box"
else
  ok "the escape hatch passes no file descriptors (no --pipe in code)"
fi

if code_only "$SAFE" | grep -c -- '--wait' >/dev/null; then
  ok "the escape hatch still waits, so the inner command's exit status is what callers see"
else
  bad "--wait is gone from $SAFE: systemd-run would return as soon as PID1 accepted the job and every caller's success check would become meaningless"
fi

if code_only "$SAFE" | grep -c 'StandardOutput=file:' >/dev/null && \
   code_only "$SAFE" | grep -c 'StandardError=file:' >/dev/null; then
  ok "output is captured via files PID1 opens itself, keeping stdout and stderr separate"
else
  bad "the StandardOutput/StandardError capture properties are gone — without them the ~10 call sites that parse stdout get nothing back"
fi

# /tmp is NOT usable: PID1 runs as init_t and writing a tmp_t file is denied on
# the RHEL family. Measured, not assumed.
CAPDIR=$(code_only "$SAFE" | sed -n 's/^const CAPTURE_DIR: &str = "\([^"]*\)".*/\1/p')
if [ -z "$CAPDIR" ]; then
  bad "CAPTURE_DIR is gone from $SAFE, so this check cannot see where captured output lands"
elif printf '%s' "$CAPDIR" | grep -cE '^/(tmp|var/tmp)(/|$)' >/dev/null; then
  bad "capture files live in $CAPDIR — PID1 (init_t) may not write tmp_t, so every unsandboxed command loses its output on RHEL"
elif grep -m1 '^ReadWritePaths=' "$UNIT" | tr ' ' '\n' | sed 's/^-//' | grep -cxF "$(printf '%s' "$CAPDIR" | sed 's#\(/var/lib/[^/]*\).*#\1#')" >/dev/null; then
  ok "capture files live under $CAPDIR, which the agent unit may read back and PID1 may write"
else
  bad "$CAPDIR is outside the agent unit's ReadWritePaths — PID1 would write the output and the agent could not read it"
fi

# The hatch is shared, but a call site could still hand-roll systemd-run.
# Do NOT look for --pipe on the same LINE as systemd-run: panel_update.rs builds
# its invocation across several .arg() calls, so a per-line match cannot see it
# and this assertion passed its own mutation test until it was rewritten. The
# flag has no other legitimate use anywhere in the agent, so require it absent
# from every code line outside the one file whose comments explain it.
stray_pipe=$(grep -rn -- '--pipe' panel/agent/src --include=*.rs \
   | grep -v '^panel/agent/src/safe_cmd.rs:' \
   | grep -vE '^[^:]*:[0-9]+:[[:space:]]*(///|//!|//|\*)')
if [ -n "$stray_pipe" ]; then
  bad "a file outside safe_cmd.rs names --pipe, which reaches the wall again through that path:${stray_pipe}"
else
  ok "no call site hand-rolls a descriptor-passing systemd-run"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
