#!/usr/bin/env bash
#
# Regression pin for the PHP version picker that could not install a PHP version.
#
# Reported by surprises29 at s288: switching a site to a version the server does
# not have failed with "PHP 8.3 is not installed or PHP-FPM is not running.
# Install it from Settings > Services…", and he said he had expected the switch
# to offer the install. Agreeing that was a gap is what this pins.
#
# Four separate things were wrong underneath one missing button:
#
#   A  The capability existed and nothing could reach it. The agent has had a
#      version-specific installer since v2.8 — deb.sury.org for Debian,
#      ppa:ondrej/php for Ubuntu, module streams for the RHEL family — and the
#      backend proxies it at /api/php/install. NOTHING in panel/frontend/src had
#      ever called it, or /php/versions, or /php/uninstall. Both pickers were a
#      hardcoded list of five options with no idea which of them existed.
#
#   B  The advice was a dead end. Settings → Services has ONE PHP tile with no
#      version on it, reporting installed if ANY version is — so on a box with
#      8.4 the operator was sent to install 8.3 on a screen showing "Installed /
#      Uninstall" and no install button at all. That tile also hits a DIFFERENT
#      agent route which takes no version and installs whatever the distro
#      offers, so following the instruction could not have worked even if the
#      button had been there.
#
#   C  The panel capped the install at 60s. `install_service_with_log` used the
#      untimed client over an operation the agent budgets 300s for, plus repo
#      configuration outside that clock. The install finished; the operator
#      watched "agent request timed out after 60s" appear in its log. This is the
#      s289 defect again — see backup-lands-pin-e2e.sh §1 — at a different call
#      site, and it affected EVERY service install, not just PHP.
#
#   D  An install could report success and leave PHP unusable. The already-
#      installed branch answered `success: true` from the PACKAGE database, while
#      the guard that sends people here tests the SOCKET. A box whose php8.3-fpm
#      was installed and stopped got "already installed" and then the same
#      failure on the very next action.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

PICKER=panel/frontend/src/components/PhpVersionPicker.tsx
SITES_TSX=panel/frontend/src/pages/Sites.tsx
DETAIL_TSX=panel/frontend/src/pages/SiteDetail.tsx
SITES_RS=panel/backend/src/routes/sites.rs
SYSTEM_RS=panel/backend/src/routes/system.rs
PHP_RS=panel/agent/src/routes/php.rs
NGINX_RS=panel/agent/src/routes/nginx.rs
PKG_RS=panel/agent/src/services/pkg.rs
MOD_RS=panel/backend/src/routes/mod.rs
for f in "$PICKER" "$SITES_TSX" "$DETAIL_TSX" "$SITES_RS" "$SYSTEM_RS" "$PHP_RS" "$NGINX_RS" "$PKG_RS" "$MOD_RS"; do
  [ -f "$f" ] || { echo "missing $f"; exit 1; }
done

# Strip // line comments from the SUBJECT FILES before matching.
#
# Not for this suite's own header — that text is never searched. It is for the
# comments in the sources, which name the very strings two negative arms forbid:
# the dead-end page in §5, and the hardcoded option list in §1. `nginx.rs` now
# carries a paragraph explaining why that page was the wrong place to send
# people, and a commented-out copy of the old `<option>` block would read as the
# block itself. Both are the trap lesson #110f recorded, on the arms most likely
# to attract an explanation.
#
# Measured: stripping changes no verdict at this commit. It is here for the edit
# that adds the comment, which is the edit it has to survive.
strip() { sed 's://.*::' "$1"; }

SRC_PICKER=$(strip "$PICKER")
SRC_SITES_TSX=$(strip "$SITES_TSX")
SRC_DETAIL_TSX=$(strip "$DETAIL_TSX")
SRC_SITES_RS=$(strip "$SITES_RS")
SRC_SYSTEM_RS=$(strip "$SYSTEM_RS")
SRC_PHP_RS=$(strip "$PHP_RS")
SRC_NGINX_RS=$(strip "$NGINX_RS")
SRC_MOD_RS=$(strip "$MOD_RS")

# Here-strings, never pipelines: `grep -q` closing a pipe kills the upstream sed
# with SIGPIPE and `pipefail` turns that into a failed arm, non-deterministically.
has()  { grep -q  -- "$2" <<< "$1"; }
hasE() { grep -qE -- "$2" <<< "$1"; }
fnbody() { awk "/pub async fn $2\(/,/^}/" <<< "$1"; }

# Every awk range extraction below anchors on an incidental spelling. If one stops
# matching, the variable is EMPTY and every arm reading it goes red with a message
# blaming the code — the failure mode is indistinguishable from the regression.
# So each extraction is checked for content before its arms run.
readable() { [ -n "$2" ] || bad "could not extract $1 — the arms reading it mean nothing"; }

echo
echo "§1 the picker knows what this server actually has (A)"

if hasE "$SRC_PICKER" 'api\.get<\{ versions: PhpVersion\[\] \}>\("/php/versions"\)'; then
  ok "the picker reads the live version list from the server"
else
  bad "the picker does not call /php/versions — it is guessing again"
fi

if has "$SRC_PICKER" 'api.post' && has "$SRC_PICKER" '"/php/install"'; then
  ok "the picker can start a version-specific install"
else
  bad "the picker cannot install — the whole point of the report"
fi

# Both entry points, because the same agent guard refuses site CREATION as well
# as a switch. Fixing only the one that was reported leaves the other identical.
for f in "$SITES_TSX" "$DETAIL_TSX"; do
  n=$(basename "$f")
  src=$(strip "$f")
  if has "$src" '<PhpVersionPicker'; then
    ok "$n uses the picker"
  else
    bad "$n still renders its own version control"
  fi
  # Negative: the dead five-option list must be gone, not merely bypassed. A
  # leftover copy is the one a later edit reaches for.
  if hasE "$src" '<option value="8\.[0-9]">PHP 8\.[0-9]</option>'; then
    bad "$n still hardcodes a PHP version list that knows nothing about the server"
  else
    ok "$n no longer carries its own hardcoded version list"
  fi
done

# The GATE, not the prop. `canInstall` appears three times in this file — the
# type, the destructure and the render condition — so matching the identifier
# anywhere passed on the two spellings that decide nothing.
if hasE "$SRC_PICKER" 'canInstall \? \('; then
  ok "the install button is rendered behind the admin gate"
else
  bad "no admin gate on the install control, while the route is admin-only"
fi

# The picker keeps a hardcoded list of its own, deliberately: it is the fallback
# for an agent that cannot answer, where offering the old five unannotated is
# better than offering nothing. Pinned so it stays a FALLBACK — reachable only
# when the load failed, never as the normal source of options.
if hasE "$SRC_PICKER" 'loadFailed$' || hasE "$SRC_PICKER" 'loadFailed\s*$'; then
  ok "the hardcoded list is guarded by the load having failed"
else
  bad "the picker's own five-version list is not conditioned on the server being unreachable"
fi

echo
echo "§2 an uninstalled version is not silently acted on"

# The switch must not fire for a version the server cannot serve — that is the
# request that produced the reported error.
if hasE "$SRC_DETAIL_TSX" 'if \(!ready'; then
  ok "the switch is held back until the chosen version is usable"
else
  bad "SiteDetail switches to whatever is selected, installed or not"
fi

# A failed switch must not leave the control asserting the version that failed —
# it states the switch happened, and the guard above then refuses to retry it
# because the selection already equals what was asked for.
if hasE "$SRC_DETAIL_TSX" 'setPhpChoice\(site\.php_version'; then
  ok "a failed switch puts the control back on what the site is running"
else
  bad "a failed switch leaves the control showing a version the site does not run"
fi

# The gate for the create form has to admit a CMS install. `handleCreate` sends
# php_version for every one of them — a one-click WordPress IS a PHP site — so a
# `!cms` gate left the commonest route to a PHP site submitting an unchecked
# default with no picker on screen.
if hasE "$SRC_SITES_TSX" '\(runtime === "php" \|\| cms\)'; then
  ok "the create form shows the picker for a CMS install too"
else
  bad "the CMS quick-install path bypasses the picker and posts an unchecked version"
fi

# Pinned as three facts about what the completion handler DOES, not as one line of
# it. ProvisionLog calls this both when the run reports "complete" — whatever its
# status — and when the stream dies having rendered nothing, so the log ending is
# not a verdict; only a fresh read of the server is. An arm spelled as the exact
# `onChange` call went red on a refactor that changed nothing about any of this.
INSTALLED_CB=$(awk '/const handleInstalled = async/,/^  \};/' <<< "$SRC_PICKER")
readable "handleInstalled" "$INSTALLED_CB"
if has "$INSTALLED_CB" 'await load()'; then
  ok "the completion handler re-reads the version list from the server"
else
  bad "the completion handler does not re-read — it is trusting the log ending"
fi
if has "$INSTALLED_CB" 'fpm_running'; then
  ok "readiness is judged on the server's own installed + running answer"
else
  bad "the handler does not check whether FPM is actually running"
fi
if hasE "$INSTALLED_CB" 'onChange\([a-zA-Z]+, true\)'; then
  ok "the version is handed back as usable from inside that handler"
else
  bad "nothing hands the freshly installed version back to the caller"
fi

echo
echo "§3 the panel outlives the agent's install budget (C)"

agent_budget=$(grep -oE 'const INSTALL_TIMEOUT: Duration = Duration::from_secs\([0-9]+\)' <<< "$(strip "$PKG_RS")" | grep -oE '[0-9]+')
if [ -n "$agent_budget" ]; then
  ok "the agent's package transaction still declares its own budget (${agent_budget}s)"
else
  bad "could not read the agent's install budget — this section's numbers need rechecking"
fi

INSTALLER=$(awk '/pub\(crate\) async fn install_service_with_log/,/^}/' <<< "$SRC_SYSTEM_RS")
readable "install_service_with_log" "$INSTALLER"
if has "$INSTALLER" 'post_long'; then
  ok "service installs go through post_long, not the 60s client"
else
  bad "install_service_with_log still uses the 60s-capped post over a multi-minute install"
fi

panel_budget=$(grep -oE 'const INSTALL_AGENT_TIMEOUT_SECS: u64 = [0-9]+' <<< "$SRC_SYSTEM_RS" | grep -oE '[0-9]+$')
if [ -n "$panel_budget" ] && [ -n "$agent_budget" ] && [ "$panel_budget" -gt "$agent_budget" ]; then
  ok "the panel allows ${panel_budget}s, above the agent's ${agent_budget}s, so the agent errors first"
else
  bad "panel budget '${panel_budget:-none}' does not exceed agent budget '${agent_budget:-none}'"
fi

PHP_INSTALL=$(fnbody "$SRC_SITES_RS" php_install)
readable "php_install" "$PHP_INSTALL"
if has "$PHP_INSTALL" 'install_service_with_log'; then
  ok "the PHP install shares the logged, budgeted install path"
else
  bad "php_install does not use the shared install path — it has its own exposure again"
fi

if has "$PHP_INSTALL" 'StatusCode::BAD_REQUEST'; then
  ok "an unknown version is refused before anything is spawned"
else
  bad "php_install spawns first and validates later, so a bad version reports as started"
fi

# A 200 stopped meaning success the moment the agent began judging an install by
# whether FPM opened its socket. Reading only the status code paints that green
# with the explanation printed underneath it.
if hasE "$INSTALLER" 'get\("success"\)' && has "$INSTALLER" '"error"'; then
  ok "a 200 carrying success:false is logged as a failure, not a completed install"
else
  bad "the install log reads the status code only — an honest failure would show as done"
fi

echo
echo "§4 an install that leaves PHP unusable is not called success (D)"

if has "$SRC_PHP_RS" 'async fn settle('; then
  ok "there is one verdict function for every install outcome"
else
  bad "no shared verdict — each install path decides for itself what success means"
fi

SETTLE=$(awk '/async fn settle\(/,/^}/' <<< "$SRC_PHP_RS")
readable "settle()" "$SETTLE"
if has "$SETTLE" 'socket_exists(version)'; then
  ok "the verdict is judged on the socket, which is what refuses a PHP vhost"
else
  bad "the verdict does not check the socket — the same wrong question as before"
fi

if has "$SETTLE" 'success: false'; then
  ok "a missing socket is reported as a failure, not as an installed package"
else
  bad "settle cannot report failure — it can only agree"
fi

# Every exit that claims success, on both families and on the already-installed
# short circuit, must go through it. One that does not is the one that lies.
settle_calls=$(grep -c 'settle(version,' <<< "$SRC_PHP_RS")
if [ "$settle_calls" -ge 3 ]; then
  ok "all $settle_calls success paths go through the same verdict"
else
  bad "only $settle_calls path(s) use the verdict — apt, rpm and already-installed all need it"
fi

ALREADY=$(awk '/if is_installed\(version\)\.await \{/,/^    \}/' <<< "$SRC_PHP_RS")
readable "the already-installed branch" "$ALREADY"
if has "$ALREADY" 'start_fpm(version)'; then
  ok "an already-installed version has its FPM unit started rather than assumed"
else
  bad "the already-installed branch still reports success without starting anything"
fi

echo
echo "§5 the message points at something that can act (B)"

if has "$SRC_NGINX_RS" 'Settings > Services'; then
  bad "the refusal still sends the operator to a page with no version-specific install"
else
  ok "the refusal no longer points at the dead end"
fi

if has "$SRC_NGINX_RS" 'PHP Version control'; then
  ok "the refusal names the control that can install the version"
else
  bad "the refusal names no remedy at all"
fi

echo
echo "§6 one version list on the panel side, not two"

if has "$SRC_SITES_RS" 'const PHP_VERSIONS'; then
  ok "the accepted versions are declared once"
else
  bad "no shared list — the two handlers can drift apart again"
fi

inline=$(grep -cE '\["8\.1", "8\.2", "8\.3", "8\.4", "8\.5"\]' <<< "$SRC_SITES_RS")
if [ "$inline" -le 1 ]; then
  ok "the literal appears $inline time(s) — the declaration itself"
else
  bad "the version literal is written out $inline times in one file"
fi

echo
echo "§7 the runtime switch the picker exists to serve (#99, s310)"

# HybridRCG, 2026-08-05: "I cant seem to find a way to change a site from html to
# php?" — his workaround was deleting the site and adding it back. Nothing in the
# tree issued `UPDATE sites SET runtime` while fourteen other site columns were
# updatable, so the answer was that there was no button, not that he had missed it.
# These arms pin the half that was promised in the thread that same morning.

if has "$SRC_MOD_RS" 'sites/{id}/runtime'; then
  ok "the runtime switch is registered as a route"
else
  bad "no /api/sites/{id}/runtime route — #99 is unreachable again"
fi

if has "$SRC_SITES_RS" 'const SWITCHABLE_RUNTIMES'; then
  ok "the switchable runtimes are declared once, as a closed list"
else
  bad "no SWITCHABLE_RUNTIMES declaration — the target runtime is unbounded"
fi

# The closed list is the whole safety argument. `document_root_for` maps proxy,
# node and python to the site directory while static and php both map to
# {site_dir}/public — so admitting a proxying runtime here turns a vhost rebuild
# into a document-root move, silently, on somebody's live site.
switchable=$(grep -A2 'const SWITCHABLE_RUNTIMES' <<< "$SRC_SITES_RS")
if [ -z "$switchable" ]; then
  bad "could not extract SWITCHABLE_RUNTIMES — the next two arms would read nothing"
elif hasE "$switchable" '"(proxy|node|python)"'; then
  bad "SWITCHABLE_RUNTIMES admits a proxying runtime — that moves the document root"
else
  ok "SWITCHABLE_RUNTIMES excludes every runtime whose document root differs"
fi

RUNTIME_FN=$(fnbody "$SRC_SITES_RS" switch_runtime)
if [ -z "$RUNTIME_FN" ]; then
  bad "switch_runtime not found — every arm below reads an empty window"
else
  if has "$RUNTIME_FN" 'php_version is required'; then
    ok "switching to php demands an explicit version"
  else
    bad "no explicit-version guard — a stale php_version column gets silently reused"
  fi

  # The ordering invariant, and the only one here that is about correctness
  # rather than reach: the vhost is written FIRST, so a refused agent write
  # leaves the row describing what is actually being served. Reversed, a site
  # whose nginx write failed would read as switched in the panel and in the API.
  put_at=$(grep -n 'agent$' <<< "$RUNTIME_FN" | head -1 | cut -d: -f1)
  [ -z "$put_at" ] && put_at=$(grep -n '\.put(' <<< "$RUNTIME_FN" | head -1 | cut -d: -f1)
  upd_at=$(grep -n 'UPDATE sites SET runtime' <<< "$RUNTIME_FN" | head -1 | cut -d: -f1)
  if [ -z "$put_at" ] || [ -z "$upd_at" ]; then
    bad "could not locate both the agent write and the UPDATE — ordering unverified"
  elif [ "$put_at" -lt "$upd_at" ]; then
    ok "the vhost is written before the row is updated (line $put_at before $upd_at)"
  else
    bad "the row is updated before the vhost — a failed nginx write reads as a switch"
  fi
fi

if has "$SRC_DETAIL_TSX" '/runtime`'; then
  ok "the site page actually calls the switch — the button #99 asked for exists"
else
  bad "no frontend caller: the endpoint is built and unreachable, which was the original complaint"
fi

# The picker reports whether a version is installed AND its FPM socket is up.
# Firing the switch anyway is what surprises29 hit at s288 in the sibling control.
if has "$SRC_DETAIL_TSX" 'runtimePhpReady'; then
  ok "the switch is held back until the chosen PHP version is really usable"
else
  bad "the switch ignores the picker's readiness — it can fire at an uninstalled version"
fi

echo
echo "§8 the direction §7 made reachable — php -> static (s311)"

# §7 pins static -> php. Every one of its arms is about that direction, and the
# defect was in the other one: v2.67.0 made `UPDATE sites SET runtime` reachable
# for the first time, and `document_root_for` maps static and php to the SAME
# {site_dir}/public — the property the switch's own reasoning cites as making it
# safe. So a docroot full of application source keeps being served, by a vhost
# that has just lost every protection.
#
# All twelve `deny all` blocks in each template live inside php preset branches.
# The static branch had root + index + try_files and nothing else, so the switch
# published wp-config.php and .git/config as plain text — verified by rendering
# both configs and serving them through a real nginx: 403 under php, 200 with the
# database password in the body under static.
#
# These arms are scoped to the STATIC BRANCH ONLY. An arm grepping either whole
# file would have been green throughout the defect's entire life, because the
# denies it looks for were present twelve times over, in the branch that was not
# broken. Strip full-line comments first: the fix carries a comment naming the
# very files it protects (lesson #149 — an arm matching the prose that narrates
# the check).
tstrip() { sed 's/^[[:space:]]*#.*//' <<< "$1"; }
hasF() { grep -qF -- "$2" <<< "$1"; }

for tpl in panel/agent/src/templates/nginx/http.conf panel/agent/src/templates/nginx/https.conf; do
  [ -f "$tpl" ] || { bad "missing $tpl — the arms below would read nothing"; continue; }

  # Bound the window on the branch's own terminator, never a fixed -A count
  # (lesson #172): the php branch below it contains all twelve denies, so a
  # window that overruns turns this section permanently, silently green.
  win=$(tstrip "$(awk '/index index\.html index\.htm/,/\{% (elif|endif)/' "$tpl")")
  base=$(basename "$tpl")

  if [ -z "$win" ] || ! hasF "$win" 'try_files'; then
    bad "$base: could not extract the static branch — every arm below is meaningless"
    continue
  fi

  # CONTROL, and it is the load-bearing one: the same window must NOT contain
  # the php handler. If it does, the extraction has bled into the php branch and
  # the two arms after it are reading the wrong code.
  if hasF "$win" 'fastcgi_pass'; then
    bad "$base: the static window bled into the php branch — its greens prove nothing"
    continue
  fi

  if hasF "$win" 'location ~ /\.(?!well-known)'; then
    ok "$base: the static branch denies dotfiles — .env and .git/ survive the switch"
  else
    bad "$base: static branch serves dotfiles — switching to static publishes .env and .git/config"
  fi

  if hasF "$win" 'location ~ \.php$'; then
    ok "$base: the static branch refuses .php source rather than serving it"
  else
    bad "$base: static branch serves .php as plain text — wp-config.php becomes readable"
  fi
done

# A template fix reaches a site only when something RE-RENDERS its vhost — a
# runtime switch, an SSL issuance, a settings change. Nothing re-renders on
# upgrade. So the site that most needs this fix, one already switched to static
# under v2.67.0, would keep serving its source after the operator updated and was
# told it was fixed. v2.68.1 retrofits the denies onto existing static vhosts in
# update.sh, the same shape as the v2.47.1 index.html migration. Without this arm
# the two template arms above are green while the field is still exposed.
UPD=scripts/update.sh
if [ ! -f "$UPD" ]; then
  bad "MISSING SUBJECT FILE: $UPD"
elif ! grep -q 'sites-enabled' "$UPD"; then
  bad "$UPD no longer touches site vhosts at all — this arm is measuring nothing"
elif grep -qF 'location ~ /\\.(?!well-known)' "$UPD" && grep -q 'nginx -t' "$UPD"; then
  ok "update.sh retrofits the denies onto existing static vhosts, validated by nginx -t"
else
  bad "nothing retrofits the denies onto vhosts already on disk — the fix reaches new renders only"
fi

# Second control, on the other side: the php branches must still carry the denies
# these arms were modelled on. A zero here would mean the greps above are matching
# a pattern this codebase no longer writes.
if [ "$(grep -cF 'deny all' panel/agent/src/templates/nginx/http.conf)" -ge 12 ]; then
  ok "the php preset branches still carry their own denies (the arms are non-vacuous)"
else
  bad "fewer denies than expected in http.conf — re-derive what this section is matching"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
