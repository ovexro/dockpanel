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
#
# ─────────────────────────────────────────────────────────────────────────────
# s338 — WHY EVERY READ BELOW GOES THROUGH A STRIPPER
#
# A regression pin greps raw source, and raw source contains prose. So an arm
# that requires a mechanism to be present is satisfied by a sentence describing
# the mechanism, and it sits green after the mechanism itself is deleted. That
# is not hypothetical here: the SELinux arm in section 6 was held up by an
# explanatory line that ships in the installer today — blanking the whole
# setsebool block left the arm green, and only blanking the explanation as well
# turned it red. The UFW-refusal arm in section 7 had the same shape: commenting
# out the three lines that make the decision changed nothing, while blanking
# those same three lines killed the arm outright.
#
# Two arms in this suite already knew. One dropped comment-looking lines before
# scanning the escape hatch; another flattened a Rust function body first,
# because an earlier version of it had matched a doc comment and passed its own
# mutation test. Both fixes were local, and both sat BELOW a dozen arms that
# still read their subjects raw — which is the s336 shape, a check that cannot
# protect what runs before it.
#
# So the strippers now sit above section 1, they choose a comment syntax by file
# EXTENSION rather than assuming one, and they BLANK a comment line instead of
# deleting it, because several arms compare line numbers taken out of that same
# stream. Sections 9-12 hold all of it in place: 9 validates the strippers on
# hermetic fixtures, 10 re-runs the s338 mutation on every push, 11 plants the
# real defect behind each negative arm and demands a fire, and 12 stops a new
# arm from being written the old way.
# ─────────────────────────────────────────────────────────────────────────────

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# ── the comment strippers, ABOVE every arm that needs them (s337/s338) ───────
#
# The comment marker is chosen by EXTENSION, never guessed. Getting it wrong is
# not symmetric: over-stripping turns a positive arm loudly red, but turns a
# NEGATIVE arm (match ⇒ bad) silently green, which is the direction that ships.
comment_marker_for() {
  case "$1" in
    *.rs|*.mjs|*.js|*.ts|*.tsx)                echo '//' ;;
    *.json)                                    echo ''   ;;  # JSON has no comments
    *)                                         echo '#'  ;;  # .sh, .service, .conf, .toml
  esac
}

# BLANK the comment, do not delete it. Several arms below take line numbers out
# of this stream and compare them against each other; a stripper that removes
# lines renumbers the file and would silently change what those arms measure.
code_lines() {
  local f="$1" m
  m=$(comment_marker_for "$f")
  if [ -z "$m" ]; then cat "$f"; return; fi
  if [ "$m" = '//' ]; then
    # A block comment is recognised ONLY where one is actually written: opening
    # at the start of a line and closing at the end of one. s294: a `/*` inside a
    # string literal (a Dockerfile's `COPY … /app/target/release/*`) opened a
    # comment that swallowed 485 lines of git_build.rs, and every ABSENCE arm
    # over it passed on code the stripper had merely removed.
    #
    # A trailing `//` is stripped, but only when the text before it has BALANCED
    # double quotes — otherwise a URL inside a string literal would lose its
    # tail, and this tree has several. Backslash escapes are skipped so an
    # escaped quote does not flip the parity.
    awk '
      /^[[:space:]]*\/\*/ &&  /\*\/[[:space:]]*$/ { print ""; next }
      /^[[:space:]]*\/\*/                         { inblk = 1; print ""; next }
      inblk { if ($0 ~ /\*\/[[:space:]]*$/) inblk = 0; print ""; next }
      /^[[:space:]]*\/\//                         { print ""; next }
      {
        line = $0; out = line; n = length(line); q = 0; i = 1
        while (i <= n) {
          c = substr(line, i, 1)
          if (c == "\\") { i += 2; continue }
          if (c == "\"") { q = 1 - q; i++; continue }
          if (q == 0 && c == "/" && substr(line, i + 1, 1) == "/") { out = substr(line, 1, i - 1); break }
          i++
        }
        print out
      }
    ' "$f"
  else
    # Shell keeps to FULL-LINE comments only, and the asymmetry with `//` above
    # is deliberate. A trailing `#` cannot be stripped safely here: parameter
    # expansion with a prefix pattern, the argument-count variable, a colour
    # literal and a sed script that edits a hash are all ordinary code. The
    # convention in this tree is that explanation goes on its own line, and §9
    # asserts the stripper leaves a hash inside a string alone.
    awk '/^[[:space:]]*#/ { print ""; next } { print }' "$f"
  fi
}

# Counted, never `grep -q`, and the reason is a trap worth spelling out. Under
# `set -o pipefail`, `code_lines f | grep -q PATTERN` reports FAILURE on a
# successful match: grep -q exits at the first hit, the producer upstream dies of
# SIGPIPE (141), and pipefail takes the pipeline's status from it. The effect is
# silent and selective — a file whose match is near the top gets dropped while
# one whose match is near the bottom survives, because the producer had already
# finished. `grep -c` consumes all of its input, so there is no early exit and no
# signal.
code_count() { local f="$1"; shift; code_lines "$f" | grep -c "$@" 2>/dev/null || true; }
has_code()   { local f="$1"; shift; [ "$(code_count "$f" "$@")" -gt 0 ]; }

# True file line numbers, because code_lines blanks rather than deletes.
code_lineno() { local f="$1"; shift; code_lines "$f" | grep -n "$@" 2>/dev/null | head -1 | cut -d: -f1 || true; }

# A recursive scan cannot use `grep -r`: that reads raw bytes off disk, so a
# commented-out call satisfies every absence arm below it. This reproduces the
# `file:line:text` output shape from the stripped stream instead, so the path
# filters those arms apply keep working unchanged. The name glob is a parameter
# because the arms differ in scope, and narrowing one of them by accident is
# exactly the silent failure this whole retrofit is about.
code_grep_tree() {
  local root="$1" glob="$2"; shift 2
  local f
  find "$root" -type f -name "$glob" -print0 2>/dev/null | while IFS= read -r -d '' f; do
    code_lines "$f" | grep -n "$@" 2>/dev/null | sed "s#^#${f}:#" || true
  done
}

# Production Rust only: comments are stripped FIRST and the test module dropped
# second. Order matters — slicing before stripping would let a comment that
# names the test attribute cut the file early. The unit tests must SPELL the
# forbidden flag in order to assert its absence, and a scanner cannot tell a
# guard from a use, so the guard would convict itself. (Rust's own tests cover
# that region.)
#
# The slice STOPS EMITTING rather than quitting, and that is the same SIGPIPE
# trap as the one above wearing different clothes: a consumer that exits early
# kills the stripper upstream with signal 13, pipefail hands that status to the
# whole pipeline, and an `if` around it reads a clean match as a failure. It
# does not fire deterministically either — a subject small enough to fit the
# pipe buffer gets away with it, so the arm that breaks is chosen by file size.
code_prod() { code_lines "$1" | awk 'fin { next } { print } /#\[cfg\(test\)\]/ { fin = 1 }'; }

# Extract a top-level Rust fn body: from its signature to the closing brace in
# column 0. Used below so assertions are about CALL SITES rather than about a
# symbol existing somewhere in the file — s265 shipped two pins that checked a
# function was *defined* and would have sat green through the exact regression
# they named. Stripping happens before the slice, so a doc comment that quotes
# the signature can no longer open a body of its own.
#
# Reads to EOF instead of exiting at the closing brace, for the SIGPIPE reason
# spelled out on code_prod: three arms here pipe this function straight into a
# grep whose status decides the arm, and an early exit made all three report the
# symbol missing from bodies that plainly contain it.
fn_body() { code_lines "$1" | awk -v pat="fn $2(" 'fin { next } index($0, pat) { inside=1 } inside { print; if ($0 == "}") fin = 1 }'; }

UNIT=panel/agent/dockpanel-agent.service
SETUP=scripts/setup.sh
UPDATE=scripts/update.sh
AGENT_SRC=panel/agent/src
SI=panel/agent/src/routes/service_installer.rs
PKG=panel/agent/src/services/pkg.rs
PHP=panel/agent/src/routes/php.rs
SEC=panel/agent/src/services/security.rs
DIAG=panel/agent/src/services/diagnostics.rs
SAFE=panel/agent/src/safe_cmd.rs

for f in "$UNIT" "$SETUP" "$UPDATE" "$SI" "$PKG" "$PHP" "$SEC" "$DIAG" "$SAFE"; do
  [ -f "$f" ] || { echo "missing file: $f"; exit 1; }
done

# ── the negative arms' patterns, hoisted ─────────────────────────────────────
#
# Each of these decides a FAILURE by finding something. §11 plants the real
# defect for every one of them and requires a fire; hoisting the pattern is what
# makes the control test the arm rather than a copy of the arm that could drift
# away from it.
NEG_APT_RE='^ReadWritePaths=.* /etc/apt( |$)'
NEG_SEDRANGE_RE="sed -i '/\^?\[\[:space:\]\]\*server \{/,/"
NEG_UFW_FX='safe_command("ufw")'
NEG_DPKG_FX='safe_command("dpkg")'
NEG_APTONLY_FX='apt_only_reason'
NEG_REDIS_RE='"(enable|start)", *"redis-server"'
NEG_DEBNODE_FX='deb.nodesource.com'
NEG_PIPE_FX='--pipe'

# The two tree-wide scans, as functions, so §11 can point them at a fixture tree
# holding the genuine defect instead of re-typing the pipeline.
scan_ufw_allow() {
  code_grep_tree "$1" '*.rs' -F -e "$NEG_UFW_FX" \
    | grep -vE 'services/firewall\.rs' \
    | grep -vE 'routes/service_installer\.rs' \
    | grep -E '"allow"' || true
}
scan_stray_pipe() {
  # `grep -r -- PATTERN dir --include=…` silently treats the include as a FILE
  # operand, which is how the original of this scan both errored on stderr and
  # covered every file rather than just Rust. Covering everything is the safer
  # of the two, so it is kept deliberately: the glob is '*'.
  code_grep_tree "$1" '*' -F -e "$NEG_PIPE_FX" | grep -v "^${2}:" || true
}

echo "── 1. No sandbox path can make the agent unstartable on a distro that lacks it ──"

# THE DURABLE PIN. /etc/apt was not a typo — it was an allow-list entry nobody
# re-checked against a non-Debian box, and two hand-maintained mirrors of this
# same list (in setup.sh and update.sh, both commented "pre-create everything the
# canonical unit lists") had missed it for as long as it existed. So rather than
# pinning that one path, require every entry to be EITHER optional-by-prefix or
# demonstrably created before the unit starts. A future entry cannot repeat this.
RWP=$(code_lines "$UNIT" | sed -n 's/^ReadWritePaths=//p' | head -1)

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
    if has_code "$SETUP" -E -e "mkdir -p( -m [0-7]+)? [^#]*(^| )${p}( |$)"; then continue; fi
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
  if has_code "$UNIT" -E -e "$NEG_APT_RE"; then
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
if has_code "$SETUP" -E -e "$NEG_SEDRANGE_RE"; then
  bad "the default-server strip is a sed range again — it ends at the first nested closing brace and corrupts nginx.conf"
else
  ok "no sed-range default-server strip in $SETUP"
fi

# Basic regex, not extended: the awk source this matches is full of parentheses
# and braces that ERE would read as grouping and repetition.
BRACE_RE='gsub(/\\{/, "{") - gsub(/\\}/, "}")'
if has_code "$SETUP" -G -e "$BRACE_RE"; then
  ok "the strip counts braces to find the block's real end"
else
  bad "the brace-counting default-server strip is gone from $SETUP — whatever replaced it must find the block end, not the first '}'"
fi

echo
echo "── 3. RHEL rebuilds do not depend on a repo upstream leaves empty ──"

if has_code "$SETUP" -F -e 'docker_repo_rhel_clone'; then
  ok "an explicit Docker repo is written for the RHEL rebuilds"
else
  bad "docker_repo_rhel_clone is gone — Rocky/Alma fall back to get.docker.com, whose rocky path serves no docker-ce and whose distro list has no almalinux"
fi

DOCKREPO_RE='download.docker.com/linux/centos/\$releasever'
if has_code "$SETUP" -G -e "$DOCKREPO_RE"; then
  ok "that repo points at the centos path, which upstream actually fills"
else
  bad "the RHEL-rebuild Docker repo no longer points at the centos path — linux/rocky/ has metadata but no docker-ce packages"
fi

# rocky and almalinux must both reach that branch. almalinux especially: detect_os
# greets it by name, so a user has every reason to expect the install to work.
RHELCASE_RE='^ *rocky\|almalinux\|centos\|rhel'
if has_code "$SETUP" -E -e "$RHELCASE_RE"; then
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
FWDEF_RE='^detect_firewall\(\) \{'
FWCALL_RE='^[[:space:]]+detect_firewall$'
if has_code "$SETUP" -E -e "$FWDEF_RE" && has_code "$SETUP" -E -e "$FWCALL_RE"; then
  ok "setup.sh detects the enforcing firewall (FW_MGR) and calls the detector from main()"
else
  bad "detect_firewall is missing or never called in $SETUP — FW_MGR stays 'none', fw_allow becomes a no-op, and the installer is back to assuming UFW"
fi

# THE DURABLE PIN for this class, same shape as §1: rather than pinning one
# call site, require every `pkg_install ufw` to sit on the branch taken only
# when no firewall is running. Checked positionally, because a textual search
# for "is it inside the case" passes for a `none)` branch that no longer exists.
UFWINST_RE='^[[:space:]]*(if run .*)?pkg_install ufw|pkg_install ufw'
ufw_installs=$(code_count "$SETUP" -E -e "$UFWINST_RE")
guarded_installs=$(code_lines "$SETUP" | awk '
  /^[[:space:]]*case "\$FW_MGR" in/ { in_case=1 }
  in_case && /^[[:space:]]*none\)/  { in_none=1 }
  in_none && /pkg_install ufw/      { n++ }
  in_none && /^[[:space:]]*;;/      { in_none=0 }
  in_case && /^[[:space:]]*esac/    { in_case=0 }
  END { print n+0 }
')
if [ "$ufw_installs" -gt 0 ] && [ "$guarded_installs" -eq "$ufw_installs" ]; then
  ok "every 'pkg_install ufw' sits on the FW_MGR=none branch ($guarded_installs/$ufw_installs)"
elif [ "$ufw_installs" -eq 0 ]; then
  ok "setup.sh never installs UFW"
else
  bad "$ufw_installs 'pkg_install ufw' in $SETUP but only $guarded_installs on the FW_MGR=none branch — installing UFW next to a running firewalld is what left the panel unreachable on every RHEL-family box"
fi

if has_code "$SETUP" -F -e 'firewall-cmd'; then
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
offenders=$(scan_ufw_allow "$AGENT_SRC")
if [ -n "$offenders" ]; then
  bad "code opens a port through ufw directly:
$offenders
    → use services::firewall::allow_tcp, which dispatches on the running firewall AND returns whether it worked"
else
  ok "port-opening goes through services/firewall.rs on every path"
fi

# Firewall STATUS must branch on the detected firewall rather than assuming ufw.
if has_code "$SEC" -F -e 'firewalld_status' && has_code "$SEC" -F -e 'firewall::detect'; then
  ok "the Security page dispatches on the running firewall instead of calling a firewalld box unfirewalled"
else
  bad "security.rs no longer dispatches on the detected firewall — on the RHEL family the Security overview reports 'no firewall' for a box that is firewalled"
fi

if has_code "$DIAG" -F -e 'firewall::detect'; then
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
if [ -n "$(code_grep_tree "$AGENT_SRC" '*.rs' -F -e "$NEG_DPKG_FX")" ]; then
  bad "an agent file calls dpkg directly — there is no dpkg on the RHEL family, so that query answers false for every package. Use services::pkg::is_installed"
else
  ok "no direct dpkg calls in the agent — package presence goes through services::pkg"
fi

if has_code "$PKG" -F -e 'PkgMgr::Rpm'; then
  ok "services::pkg dispatches on the box's real package database"
else
  bad "services::pkg no longer handles rpm — every package query is Debian-only again"
fi

echo
echo "── 6. SELinux is accounted for, not discovered by the operator ──"

# With SELinux Enforcing (the RHEL-family default) nginx may not open a socket
# to the API, so every request answered 502 — including from the box itself.
# The denial is dontaudit'ed: nothing in the journal, nothing in ausearch.
#
# s338: this arm is the reason the whole file was retrofitted. Blanking the
# entire boolean-setting block left it green, because the block's own header
# sentence still named the boolean. It reads CODE now.
SELINUX_FX='httpd_can_network_connect'
if has_code "$SETUP" -F -e "$SELINUX_FX"; then
  ok "setup.sh sets httpd_can_network_connect, without which the panel answers 502 on Enforcing systems"
else
  bad "$SETUP no longer sets httpd_can_network_connect — nginx cannot reach the API under Enforcing SELinux and every request 502s with no log line to explain it"
fi

# Existing broken boxes cannot be fixed from the panel, because the panel is
# what is unreachable. update.sh is the only path in.
if has_code "$UPDATE" -F -e "$SELINUX_FX" && has_code "$UPDATE" -F -e 'firewall-cmd'; then
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
#
# Scope is every file under the agent tree, not only Rust, because that is what
# the original of this scan covered and narrowing it would fail in silence.
if [ -n "$(code_grep_tree "$AGENT_SRC" '*' -F -e "$NEG_APTONLY_FX")" ]; then
  bad "apt_only_reason still has callers — s266 replaced it, so any survivor is a handler refusing for a reason that is no longer true"
else
  ok "the blanket apt-only refusal is gone everywhere, not just where it was convenient"
fi

# THE ONE REFUSAL THAT MUST SURVIVE. UFW is installable from EPEL, so "the
# package exists" is true and beside the point: the RHEL family boots with
# firewalld enforcing, and s265's outage was setup.sh installing ufw beside it
# and opening 80/443 in the filter nobody consults — panel unreachable, ACME
# challenge unfetchable, no certificate. Giving this button a dnf path would
# make that outage reachable by one click.
#
# s338: commenting out the three lines that make this decision used to leave the
# arm green. fn_body strips first now, so a disabled call is not a call.
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
PKGROUTE_RE='pkg::(install|install_available|add_repo)'
total=0 routed=0 unrouted=''
for f in $INSTALLERS; do
  total=$((total+1))
  body=$(fn_body "$SI" "$f")
  if printf '%s' "$body" | grep -cE "$PKGROUTE_RE" >/dev/null; then
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
   ! printf '%s' "$REDIS_BODY" | grep -cE "$NEG_REDIS_RE" >/dev/null; then
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
if has_code "$PHP" -F -e 'installed_php_version'; then
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
# The hand-rolled comment filter this line used to carry is gone: fn_body now
# blanks comments for every subject, so it was a second, weaker copy.
MAPS=$( { fn_body "$PKG" rpm_name; fn_body "$PKG" php_rpm_name; } | tr '\n' ' ')
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
   printf '%s' "$ADDREPO" | grep -c "$NEG_DEBNODE_FX" >/dev/null && \
   ! has_code "$SI" -F -e "$NEG_DEBNODE_FX"; then
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
# These assertions read CODE ONLY. safe_cmd.rs discusses the forbidden flag at
# length in its doc comments precisely because it must never use it again, so a
# grep over the raw file would match the explanation and sit green through the
# regression it names — the trap that let one of s266's own pins pass its
# mutation test.
if code_prod "$SAFE" | grep -cF -e "$NEG_PIPE_FX" >/dev/null; then
  bad "$SAFE passes --pipe to systemd-run again — that is THE WALL, and it makes every package operation fail on every SELinux box"
else
  ok "the escape hatch passes no file descriptors (no --pipe in code)"
fi

if code_prod "$SAFE" | grep -cF -e '--wait' >/dev/null; then
  ok "the escape hatch still waits, so the inner command's exit status is what callers see"
else
  bad "--wait is gone from $SAFE: systemd-run would return as soon as PID1 accepted the job and every caller's success check would become meaningless"
fi

if code_prod "$SAFE" | grep -c 'StandardOutput=file:' >/dev/null && \
   code_prod "$SAFE" | grep -c 'StandardError=file:' >/dev/null; then
  ok "output is captured via files PID1 opens itself, keeping stdout and stderr separate"
else
  bad "the StandardOutput/StandardError capture properties are gone — without them the ~10 call sites that parse stdout get nothing back"
fi

# /tmp is NOT usable: PID1 runs as init_t and writing a tmp_t file is denied on
# the RHEL family. Measured, not assumed.
CAPDIR=$(code_prod "$SAFE" | sed -n 's/^const CAPTURE_DIR: &str = "\([^"]*\)".*/\1/p')
if [ -z "$CAPDIR" ]; then
  bad "CAPTURE_DIR is gone from $SAFE, so this check cannot see where captured output lands"
elif printf '%s' "$CAPDIR" | grep -cE '^/(tmp|var/tmp)(/|$)' >/dev/null; then
  bad "capture files live in $CAPDIR — PID1 (init_t) may not write tmp_t, so every unsandboxed command loses its output on RHEL"
elif printf '%s' "$RWP" | tr ' ' '\n' | sed 's/^-//' | grep -cxF "$(printf '%s' "$CAPDIR" | sed 's#\(/var/lib/[^/]*\).*#\1#')" >/dev/null; then
  ok "capture files live under $CAPDIR, which the agent unit may read back and PID1 may write"
else
  bad "$CAPDIR is outside the agent unit's ReadWritePaths — PID1 would write the output and the agent could not read it"
fi

# The hatch is shared, but a call site could still hand-roll systemd-run.
# Do NOT look for the flag on the same LINE as systemd-run: panel_update.rs
# builds its invocation across several .arg() calls, so a per-line match cannot
# see it and this assertion passed its own mutation test until it was rewritten.
# The flag has no other legitimate use anywhere in the agent, so require it
# absent from every code line outside the one file whose comments explain it.
stray_pipe=$(scan_stray_pipe "$AGENT_SRC" "$SAFE")
if [ -n "$stray_pipe" ]; then
  bad "a file outside safe_cmd.rs names --pipe, which reaches the wall again through that path:${stray_pipe}"
else
  ok "no call site hand-rolls a descriptor-passing systemd-run"
fi

echo
echo "── 9. the strippers strip comments, and nothing else (s338) ──"

# Everything above now depends on code_lines. A stripper that under-strips
# restores the whole defect silently; one that OVER-strips turns every negative
# arm green on broken code. So it gets its own fixtures, and they are hermetic —
# no repository file is involved, because a fixture that shares a subject with
# the thing it validates cannot tell the two apart.
FIXDIR=$(mktemp -d)
trap 'rm -rf "$FIXDIR"' EXIT

cat > "$FIXDIR/f.sh" <<'FIX'
#!/usr/bin/env bash
# TOKENWORD lives here as a full-line comment
run_the TOKENWORD --now
echo "a literal hash # inside a string keeps TOKENSTR"
FIX

cat > "$FIXDIR/f.rs" <<'FIX'
// TOKENWORD in a line comment
/*
 * TOKENWORD in a block comment
 */
let live = call(TOKENWORD);
let trail = other(); // TOKENWORD trailing
let url = "https://example.invalid/TOKENURL";
let glob = "COPY /app/target/release/*";
let after_glob = keep(TOKENAFTER);
FIX

printf '{ "name": "x", "note": "TOKENWORD" }\n' > "$FIXDIR/f.json"

if [ "$(code_count "$FIXDIR/f.sh" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "shell: a full-line '#' comment carrying the token is blanked (1 code hit, not 2)"
else
  bad "shell: expected exactly 1 code hit for TOKENWORD, got $(code_count "$FIXDIR/f.sh" -F -e 'TOKENWORD')"
fi

if [ "$(code_count "$FIXDIR/f.sh" -F -e 'TOKENSTR')" -eq 1 ]; then
  ok "shell: a '#' inside a string literal is left alone (no trailing-comment stripping)"
else
  bad "shell: the stripper ate a '#' inside a string — a negative arm would now pass on broken code"
fi

if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "rust: '//' line, '/* */' block and trailing '//' all blanked (1 code hit of 4)"
else
  bad "rust: expected exactly 1 code hit for TOKENWORD, got $(code_count "$FIXDIR/f.rs" -F -e 'TOKENWORD')"
fi

if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENURL')" -eq 1 ]; then
  ok "rust: '//' inside a quoted string survives — a URL is not a comment"
else
  bad "rust: the stripper cut a line at the '//' of a URL inside a string literal"
fi

# s294: a `/*` inside a string literal opened a block comment that ran to the
# next `*/` and deleted 485 lines of git_build.rs. The guard is that a block is
# recognised only where one is WRITTEN — opening at line start.
if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENAFTER')" -eq 1 ]; then
  ok "rust: a '/*' inside a string literal does not open a block (the s294 guard holds)"
else
  bad "rust: a string containing '/*' swallowed the code after it — the s294 stripper defect is back"
fi

if [ "$(code_count "$FIXDIR/f.json" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "json: nothing is stripped, because JSON has no comment syntax"
else
  bad "json: the stripper altered a JSON file, which has no comments to remove"
fi

# §7 and §8 read line numbers out of this stream and compare them. Deleting
# rather than blanking would renumber every subject and change what they measure.
LINEDRIFT=""
for f in "$UNIT" "$SETUP" "$UPDATE" "$SI" "$PKG" "$PHP" "$SEC" "$DIAG" "$SAFE"; do
  a=$(wc -l < "$f"); b=$(code_lines "$f" | wc -l)
  [ "$a" != "$b" ] && LINEDRIFT="$LINEDRIFT $f($a≠$b)"
done
if [ -z "$LINEDRIFT" ]; then
  ok "blanking preserves the line count of all 9 subjects, so line numbers stay true"
else
  bad "code_lines changed the line count of:$LINEDRIFT — the ordering arms are now measuring the wrong lines"
fi

# The most direct over-strip control available for the shell subjects: if the
# stripper removed part of a command, the result stops being a valid script.
PARSEFAIL=""
for f in "$SETUP" "$UPDATE"; do
  code_lines "$f" | bash -n 2>/dev/null || PARSEFAIL="$PARSEFAIL $f"
done
if [ -z "$PARSEFAIL" ]; then
  ok "every shell subject still parses after blanking, so no command was cut in half"
else
  bad "these no longer parse after blanking, so the stripper removed code:$PARSEFAIL"
fi

echo
echo "── 10. every pinned pattern is live CODE, and a comment cannot satisfy it (s338) ──"

# This is the s338 mutation, encoded. A round of mutation testing done by hand
# dies with the session that did it; the arms below re-run it on every push.
#
# For each subject: assert every pattern this suite pins is present in CODE (a
# pattern that has quietly become prose-only would otherwise turn red later and
# look like a test bug rather than the defect it is), then turn each matching
# code line into a comment in a throwaway copy and assert the count falls to
# zero. If it does not, some arm above is still reading the file raw.
PINNED=(
  "$UNIT|-E|^ReadWritePaths="
  "$SETUP|-G|$BRACE_RE"
  "$SETUP|-F|docker_repo_rhel_clone"
  "$SETUP|-G|$DOCKREPO_RE"
  "$SETUP|-E|$RHELCASE_RE"
  "$SETUP|-E|$FWDEF_RE"
  "$SETUP|-E|$FWCALL_RE"
  "$SETUP|-E|$UFWINST_RE"
  "$SETUP|-F|case \"\$FW_MGR\" in"
  "$SETUP|-F|firewall-cmd"
  "$SETUP|-F|$SELINUX_FX"
  "$UPDATE|-F|$SELINUX_FX"
  "$UPDATE|-F|firewall-cmd"
  "$SI|-E|fn install_ufw\("
  "$SI|-F|ufw_refusal_reason"
  "$SI|-F|pkg::install"
  "$SI|-E|$PKGROUTE_RE"
  "$SI|-F|enable_php_stream"
  "$SI|-F|service_name"
  "$SI|-F|schema not found"
  "$PKG|-F|PkgMgr::Rpm"
  "$PKG|-F|firewall::detect"
  "$PKG|-F|rpm.nodesource.com"
  "$PKG|-F|$NEG_DEBNODE_FX"
  "$PKG|-E|fn ufw_refusal_reason\("
  "$PKG|-E|fn rpm_name\("
  "$PKG|-E|fn php_rpm_name\("
  "$PKG|-E|fn add_repo\("
  "$PHP|-F|php_streams"
  "$PHP|-F|installed_php_version"
  "$PHP|-E|fn install_version\("
  "$SEC|-F|firewalld_status"
  "$SEC|-F|firewall::detect"
  "$DIAG|-F|firewall::detect"
  "$SAFE|-F|--wait"
  "$SAFE|-F|StandardOutput=file:"
  "$SAFE|-F|StandardError=file:"
  "$SAFE|-E|^const CAPTURE_DIR"
)

# Turn every CODE line matching the pattern into a comment, leaving the text.
# That is the defect this section is about: the mechanism removed, the
# explanation left behind.
#
# The copy KEEPS the original's extension, and that is load-bearing rather than
# tidy: comment_marker_for dispatches on it, so a scratch file called `mutated`
# is read as shell no matter what it holds — and every Rust subject would then
# report the stripper broken when what was broken was the temp filename.
comment_out_matches() {
  local src="$1" dst="$2" mode="$3" pat="$4" marker
  marker=$(comment_marker_for "$src")
  code_lines "$src" | grep -n "$mode" -e "$pat" 2>/dev/null | cut -d: -f1 > "$FIXDIR/hits" || true
  # Keyed on FILENAME, not `NR == FNR`: with an EMPTY hit list (a pin that has gone
  # prose-only) the first record of the SUBJECT also satisfies NR == FNR, the whole
  # file is consumed as hit numbers, and the empty copy then reads as "the stripper
  # is not reaching this subject" when the truth is "nothing left to comment out".
  awk -v marker="$marker" -v hitsfile="$FIXDIR/hits" \
      'FILENAME == hitsfile { hit[$1]=1; next } { if (FNR in hit) print marker " " $0; else print }' \
      "$FIXDIR/hits" "$src" > "$dst"
}

PIN_SUBJECTS=$(printf '%s\n' "${PINNED[@]}" | cut -d'|' -f1 | sort -u)
for subj in $PIN_SUBJECTS; do
  dead="" survived=""
  for entry in "${PINNED[@]}"; do
    f=${entry%%|*}; rest=${entry#*|}; mode=${rest%%|*}; pat=${rest#*|}
    [ "$f" = "$subj" ] || continue
    [ "$(code_count "$f" "$mode" -e "$pat")" -gt 0 ] || dead="$dead [$pat]"
    mutated="$FIXDIR/mutated_$(basename "$f")"
    comment_out_matches "$f" "$mutated" "$mode" "$pat"
    # Assert the plant LANDED before reading the result: an empty copy would
    # score zero and read exactly like a stripper that works.
    if [ ! -s "$mutated" ]; then
      survived="$survived [$pat→copy-empty]"
      continue
    fi
    left=$(code_lines "$mutated" | grep -c "$mode" -e "$pat" 2>/dev/null || true)
    [ "$left" -eq 0 ] || survived="$survived [$pat→$left]"
  done

  if [ -z "$dead" ]; then
    ok "$subj: every pattern this suite pins is present in CODE, not only in prose"
  else
    bad "$subj: pinned pattern(s) present only as prose —$dead — the arm reading it is passing on a comment"
  fi

  if [ -z "$survived" ]; then
    ok "$subj: commenting out the matching code lines drives every pinned pattern to zero"
  else
    bad "$subj: pattern(s) still counted after their code lines were commented out —$survived — the stripper is not reaching this subject"
  fi
done

echo
echo "── 11. every NEGATIVE arm still fires on a real defect (s338) ──"

# The quiet direction. A negative arm fails by FINDING something, so narrowing
# what it can see cannot turn it red — it turns it green on genuinely broken
# code, and nothing in the run says so. Section 10 proves the strippers reach
# each subject; these arms prove they do not reach too far, by writing the
# actual defect into a copy and requiring the arm's own pattern to catch it.
#
# One arm per negative arm above, in the same order.
NEGFIRE=0

mkdir -p "$FIXDIR/tree"

cp "$UNIT" "$FIXDIR/u.service"
printf 'ReadWritePaths=/var/www /etc/apt\n' >> "$FIXDIR/u.service"
if has_code "$FIXDIR/u.service" -E -e "$NEG_APT_RE"; then
  NEGFIRE=$((NEGFIRE+1)); ok "§1's unprefixed-/etc/apt arm still fires when the entry is really in the unit"
else
  bad "§1's unprefixed-/etc/apt arm no longer sees a real entry — the stripper is eating unit directives"
fi

cp "$SETUP" "$FIXDIR/s.sh"
printf "sed -i '/^[[:space:]]*server {/,/^[[:space:]]*}/d' /etc/nginx/nginx.conf\n" >> "$FIXDIR/s.sh"
if has_code "$FIXDIR/s.sh" -E -e "$NEG_SEDRANGE_RE"; then
  NEGFIRE=$((NEGFIRE+1)); ok "§2's sed-range arm still fires when the range edit is really in code"
else
  bad "§2's sed-range arm no longer sees a real range edit — the stripper is eating shell code"
fi

cat > "$FIXDIR/tree/ports.rs" <<'FIX'
pub async fn open_mail_ports() {
    let _ = safe_command("ufw").arg("allow").arg("25/tcp").run().await;
}
FIX
if [ -n "$(scan_ufw_allow "$FIXDIR/tree")" ]; then
  NEGFIRE=$((NEGFIRE+1)); ok "§4's direct-ufw arm still fires when a port is really opened through ufw"
else
  bad "§4's direct-ufw arm no longer sees a real ufw allow — the tree scan is stripping code, not comments"
fi

cat > "$FIXDIR/tree/pkgquery.rs" <<'FIX'
pub async fn is_installed(name: &str) -> bool {
    safe_command("dpkg").arg("-l").arg(name).run().await.is_ok()
}
FIX
if [ -n "$(code_grep_tree "$FIXDIR/tree" '*.rs' -F -e "$NEG_DPKG_FX")" ]; then
  NEGFIRE=$((NEGFIRE+1)); ok "§5's direct-dpkg arm still fires when a real dpkg call is in code"
else
  bad "§5's direct-dpkg arm no longer sees a real dpkg call — the tree scan is stripping code, not comments"
fi

cat > "$FIXDIR/tree/refusal.rs" <<'FIX'
pub async fn install_thing() -> Result<(), String> {
    if let Some(why) = pkg::apt_only_reason().await { return Err(why); }
    Ok(())
}
FIX
if [ -n "$(code_grep_tree "$FIXDIR/tree" '*' -F -e "$NEG_APTONLY_FX")" ]; then
  NEGFIRE=$((NEGFIRE+1)); ok "§7's stale-refusal arm still fires when a real caller survives"
else
  bad "§7's stale-refusal arm no longer sees a real caller — the tree scan is stripping code, not comments"
fi

cat > "$FIXDIR/redis.rs" <<'FIX'
pub async fn install_redis() -> Result<(), String> {
    let unit = pkg::service_name("redis-server").await;
    systemctl(&["enable", "redis-server"]).await;
    Ok(())
}
FIX
if printf '%s' "$(fn_body "$FIXDIR/redis.rs" install_redis)" | grep -cE "$NEG_REDIS_RE" >/dev/null; then
  NEGFIRE=$((NEGFIRE+1)); ok "§8's hardcoded-redis-unit arm still fires when the Debian unit name is really enabled"
else
  bad "§8's hardcoded-redis-unit arm no longer sees a real hardcoded unit — fn_body is stripping code, not comments"
fi

cp "$SI" "$FIXDIR/si.rs"
printf '\nconst NODE_SETUP: &str = "https://deb.nodesource.com/setup_22.x";\n' >> "$FIXDIR/si.rs"
if has_code "$FIXDIR/si.rs" -F -e "$NEG_DEBNODE_FX"; then
  NEGFIRE=$((NEGFIRE+1)); ok "§8's hardcoded-deb-repo arm still fires when an installer really names the deb script"
else
  bad "§8's hardcoded-deb-repo arm no longer sees a real deb repo URL — the stripper cut a string literal"
fi

# The plant must land BEFORE the test module, because code_prod stops there.
# Appending would put it out of scope and the silence would look like a working
# stripper rather than a misplaced fixture.
awk '/^#\[cfg\(test\)\]/ && !seen { print "    push(\"--pipe\".into());"; seen=1 } { print }' \
    "$SAFE" > "$FIXDIR/safe.rs"
if code_prod "$FIXDIR/safe.rs" | grep -cF -e "$NEG_PIPE_FX" >/dev/null; then
  NEGFIRE=$((NEGFIRE+1)); ok "the WALL arm still fires when the forbidden flag is really in production code"
else
  bad "the WALL arm no longer sees a real use of the forbidden flag — code_prod is cutting production code"
fi

cat > "$FIXDIR/tree/other.rs" <<'FIX'
pub fn build() -> Command {
    let mut c = Command::new("systemd-run");
    c.arg("--pipe");
    c
}
FIX
if [ -n "$(scan_stray_pipe "$FIXDIR/tree" "$FIXDIR/tree/none.rs")" ]; then
  NEGFIRE=$((NEGFIRE+1)); ok "the stray-flag arm still fires when another file really hand-rolls the invocation"
else
  bad "the stray-flag arm no longer sees a real hand-rolled invocation — the tree scan is stripping code, not comments"
fi

echo
echo "── 12. no arm in this suite may read a subject raw again (s338) ──"

# The structural guard. §10 proves today's arms are comment-blind; this one
# stops a NEW arm from being written the old way, which is exactly how the
# escape-hatch section came to differ from the ten above it in the first place.
SELF=tests/rpm-install-pin-e2e.sh

# Assembled at runtime from a variable, so this line does not contain the shape
# it searches for and the scan cannot match itself.
#
# The boundary before the tool name is load-bearing: the stripping tree-scanner
# defined at the top of this file carries that name inside its own, so without
# it every legitimate call to the scanner convicted itself and the two arms that
# use it were reported as raw reads.
SUBJ_VARS='UNIT|SETUP|UPDATE|SI|PKG|PHP|SEC|DIAG|SAFE|AGENT_SRC'
RAW_ARMS=$(code_lines "$SELF" | grep -nE "(^|[^_[:alnum:]])grep[^|;]*\"\\\$($SUBJ_VARS)[\"/]" || true)
if [ -z "$RAW_ARMS" ]; then
  ok "no arm greps a subject file directly; every read goes through code_lines"
else
  bad "these lines read a subject raw instead of through code_lines: $(printf '%s' "$RAW_ARMS" | cut -d: -f1 | tr '\n' ' ')"
fi

# s336 #364: a check placed below an early exit inherits that exit's blind spot.
# The strippers were the same shape in the suite this one copied them from —
# defined near the end, used by nothing above. Assert the definition precedes
# the first section rather than trusting it.
DEF_LINE=$(code_lineno "$SELF" -E -e '^code_lines\(\) \{')
SEC1_LINE=$(code_lineno "$SELF" -F -e '── 1. No sandbox path can make')
if [ -n "$DEF_LINE" ] && [ -n "$SEC1_LINE" ] && [ "$DEF_LINE" -lt "$SEC1_LINE" ]; then
  ok "code_lines is defined (line $DEF_LINE) above the first section (line $SEC1_LINE)"
else
  bad "code_lines is no longer defined above section 1 — the arms above its definition are reading raw source again (def=${DEF_LINE:-none} §1=${SEC1_LINE:-none})"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
printf 'negative arms: 9   negative controls fired: %d\n' "$NEGFIRE"
[ "$FAIL" -eq 0 ]
