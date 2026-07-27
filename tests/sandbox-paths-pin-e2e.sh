#!/usr/bin/env bash
# Regression pins for s269 — the "-" prefix class in the agent's ReadWritePaths.
#
# systemd's `-` prefix means "bind this path IF IT EXISTS". The two halves of
# the unit's list therefore fail in opposite ways:
#
#   S1  An UNPREFIXED entry that is missing fails the namespace mount and the
#       agent does not start. Loud, immediate, self-correcting.
#   S2  A `-`-PREFIXED entry that is missing is skipped silently. The unit
#       starts, systemd reports success, and every write beneath that path
#       fails with `Read-only file system` for the lifetime of the namespace.
#       Verified directly against systemd at s269: with the directory absent the
#       unit's own ExecStart got `mkdir: cannot create directory: Read-only file
#       system` while `systemctl show -p Result` still said `success`. Creating
#       the directory afterwards does not rescue a RUNNING service — the mount
#       namespace is fixed at start.
#
# That is why the installers pre-create these directories, and why the list they
# create is load-bearing rather than belt-and-braces. It was hand-copied into
# setup.sh AND update.sh, and it drifted: /var/spool/cron reached the unit and
# setup.sh at s268 but never update.sh's loop — so an upgraded box without a
# cron spool got a silently unwritable one, defeating on exactly those boxes the
# "existing installs recover on upgrade" property s268's cron fix was built for.
#
# The repair is to DERIVE the list from the unit instead of mirroring it. These
# pins hold the derivation in place; a future hand-written list fails here.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

UNIT=panel/agent/dockpanel-agent.service
SETUP=scripts/setup.sh
UPDATE=scripts/update.sh

for f in "$UNIT" "$SETUP" "$UPDATE"; do
  [ -f "$f" ] || { echo "missing file: $f"; exit 1; }
done

# The unit is the single source of truth. Read it the same way the installers do.
RWP_LINE=$(grep '^ReadWritePaths=' "$UNIT" | head -1)
ALL=$(printf '%s' "$RWP_LINE" | cut -d= -f2- | tr ' ' '\n' | sed 's/^-//' | grep '^/' || true)
DASH=$(printf '%s' "$RWP_LINE" | cut -d= -f2- | tr ' ' '\n' | grep '^-/' | sed 's/^-//' || true)

echo "── 1. the unit still declares a sandbox worth mirroring ──"

if [ -n "$RWP_LINE" ]; then
  ok "the agent unit declares ReadWritePaths ($(printf '%s\n' "$ALL" | grep -c . ) entries, $(printf '%s\n' "$DASH" | grep -c . ) of them '-'-prefixed)"
else
  bad "the agent unit has no ReadWritePaths — if the sandbox was dropped, delete this suite deliberately rather than letting it pass"
fi

# The `-` prefix is the silent half. If it ever disappears entirely the class is
# gone, but so is tolerance for optional services, so flag it as a real change.
if [ -n "$DASH" ]; then
  ok "'-'-prefixed entries exist, so pre-creation is load-bearing"
else
  bad "no '-'-prefixed entries remain — every path is now mandatory; confirm that is intended"
fi

if grep -qE '^ProtectSystem=strict' "$UNIT"; then
  ok "ProtectSystem=strict — an unbound path really is read-only"
else
  bad "ProtectSystem is no longer strict; the sandbox these paths carve out may not exist"
fi

echo
echo "── 2. both installers DERIVE the list rather than mirroring it ──"

for f in "$SETUP" "$UPDATE"; do
  if grep -q 'agent_rwp_dirs()' "$f"; then
    ok "$f defines agent_rwp_dirs"
  else
    bad "$f no longer derives the directory list from the unit — a hand-written list is how this drifted"
  fi
  # Defining it and never calling it would pass a naive pin while restoring the bug.
  if [ "$(grep -c 'agent_rwp_dirs' "$f")" -ge 2 ]; then
    ok "$f actually calls it"
  else
    bad "$f defines agent_rwp_dirs but never calls it"
  fi
done

# Under `set -euo pipefail` a grep that matches nothing kills the script, and
# these run at install time where the failure is worst.
for f in "$SETUP" "$UPDATE"; do
  if grep -A 6 'agent_rwp_dirs()' "$f" | grep -qF '|| true'; then
    ok "$f's helper tolerates a no-match under pipefail"
  else
    bad "$f's helper can abort the installer when the grep matches nothing (set -euo pipefail)"
  fi
done

echo
echo "── 3. the derivation reproduces the unit exactly ──"

# Run the real helper, as defined in the shipped script, against the real unit.
derive() {
  bash -c '
    set -euo pipefail
    AGENT_SRC="$1"
    '"$(sed -n '/^agent_rwp_dirs() {/,/^}/p' "$SETUP")"'
    agent_rwp_dirs
  ' _ "$(dirname "$UNIT")"
}
DERIVED=$(derive | sort || true)
EXPECTED=$(printf '%s\n' "$ALL" | sort)

if [ "$DERIVED" = "$EXPECTED" ]; then
  ok "the helper yields exactly the unit's $(printf '%s\n' "$EXPECTED" | grep -c .) paths, '-' stripped"
else
  bad "the helper's output does not match the unit: $(diff <(printf '%s\n' "$EXPECTED") <(printf '%s\n' "$DERIVED") | tr '\n' ' ')"
fi

# The '-' prefix must be STRIPPED, not carried through — `mkdir -p -/etc/php`
# would be read as a flag and the directory would never be created.
if printf '%s\n' "$DERIVED" | grep -q '^-'; then
  bad "derived paths still carry the '-' prefix; mkdir would treat it as an option"
else
  ok "the '-' prefix is stripped from every derived path"
fi

echo
echo "── 4. the specific drift that started this, and the ordering that makes it work ──"

# /var/spool/cron is the path that drifted. Pinning it by name keeps the actual
# regression down even if the derivation is rewritten later.
for f in "$SETUP" "$UPDATE"; do
  if printf '%s\n' "$DERIVED" | grep -qx '/var/spool/cron'; then
    ok "$f covers /var/spool/cron (via the derivation)"
  else
    bad "$f does not cover /var/spool/cron — the s268 cron fix stops landing on upgraded boxes"
  fi
done

if printf '%s\n' "$ALL" | grep -qx '/var/spool/cron'; then
  ok "the unit still grants the cron spool, so crontabs remain writable"
else
  bad "/var/spool/cron left the unit — the agent cannot write crontabs at all"
fi

# The directories must exist BEFORE the agent restarts, or the bind is skipped
# for the new namespace and the fix is inert until the next restart.
MK_LINE=$(grep -n 'for d in \$(agent_rwp_dirs)' "$UPDATE" | head -1 | cut -d: -f1)
CP_LINE=$(grep -n 'cp "\$AGENT_SRC/dockpanel-agent.service"' "$UPDATE" | head -1 | cut -d: -f1)
if [ -n "$MK_LINE" ] && [ -n "$CP_LINE" ] && [ "$MK_LINE" -lt "$CP_LINE" ]; then
  ok "update.sh creates the directories (line $MK_LINE) before installing the unit (line $CP_LINE)"
else
  bad "update.sh's directory creation no longer precedes the unit install — the new namespace can be built before the paths exist"
fi

echo
echo "── 5. the THIRD installer, which these pins could not see (s271) ──"

# Sections 1-4 pin the unit, setup.sh and update.sh. install-agent.sh — the only
# documented way to add a remote server, and therefore the installer that built
# the entire fleet — was never named here, so it hand-wrote its own unit for
# eighteen releases with the sandbox switched off and nothing failed. A suite
# that covers two of three copies reads as coverage.
INSTALL_AGENT=scripts/install-agent.sh
SELF_UPDATE=scripts/agent-self-update.sh
AGENT_UNIT_RS=panel/agent/src/services/agent_unit.rs
AGENT_MAIN=panel/agent/src/main.rs

for f in "$INSTALL_AGENT" "$SELF_UPDATE" "$AGENT_UNIT_RS" "$AGENT_MAIN"; do
  [ -f "$f" ] || { echo "missing file: $f"; exit 1; }
done

# 5a. No hand-written unit anywhere in the fleet installer. Each switch is
# checked by name: the copy set all four, and a partial restoration is still a
# regression.
for directive in NoNewPrivileges ProtectSystem ProtectHome PrivateTmp; do
  if grep -qE "^[[:space:]]*${directive}=no" "$INSTALL_AGENT"; then
    bad "$INSTALL_AGENT sets ${directive}=no again — that is the shape every fleet member ran unsandboxed under"
  else
    ok "$INSTALL_AGENT does not disable ${directive}"
  fi
done

if grep -qE '^\[Service\]' "$INSTALL_AGENT"; then
  bad "$INSTALL_AGENT contains a unit body again — the third copy is back"
else
  ok "$INSTALL_AGENT contains no unit body at all"
fi

# 5b. It takes the unit from the binary it just downloaded. Anchored on the flag
# followed by the redirect, not on the flag as a substring: `--print-unit` alone
# still matches `--print-unit-x`, and a pin that survives a rename is worth
# nothing (s270's `const RSPAMD_MILTER`).
if grep -qF -- '--print-unit >' "$INSTALL_AGENT"; then
  ok "$INSTALL_AGENT obtains the unit from the agent binary"
else
  bad "$INSTALL_AGENT no longer asks the binary for the unit — it is writing one from somewhere"
fi

# The flag is a contract between three files. If main.rs stops answering to the
# exact string the installers send, install-agent.sh refuses every install and
# the self-update silently stops refreshing the unit.
if grep -qF '"--print-unit"' "$AGENT_MAIN"; then
  ok "the agent answers to the same flag the installers invoke"
else
  bad "main.rs no longer handles \"--print-unit\" — the installers ask for a unit nothing emits"
fi

# 5c. And REFUSES rather than falling back. A fallback here is what the whole
# defect was: a locally-invented unit that nobody compared to the real one.
if grep -A 8 -- '--print-unit' "$INSTALL_AGENT" | grep -qE '^[[:space:]]*exit 1'; then
  ok "$INSTALL_AGENT exits non-zero when the binary cannot emit a unit"
else
  bad "$INSTALL_AGENT does not fail when --print-unit fails — an absent unit must be an error, never a guess"
fi

# 5d. The derived directory list, same rule as setup.sh/update.sh in section 2.
# /etc/nginx is UNPREFIXED and an agent-only box has no nginx, so without this
# the hardened unit makes every fleet member unstartable.
if grep -q 'ReadWritePaths=' "$INSTALL_AGENT" && grep -qF "sed 's/^-//'" "$INSTALL_AGENT"; then
  ok "$INSTALL_AGENT derives its mkdir list from the installed unit, prefix stripped"
else
  bad "$INSTALL_AGENT does not derive the writable-path directories — /etc/nginx alone would make the agent unstartable on a fleet member"
fi

# Anchored on the guarded grep ITSELF, not on `|| true` appearing anywhere in
# the next few lines — the mkdir loop below it carries its own `|| true`, so the
# window form passed with the guard deleted. Found by mutating it (#97h).
if grep -qF "{ grep '^/' || true; }" "$INSTALL_AGENT"; then
  ok "$INSTALL_AGENT's derivation tolerates a no-match under pipefail"
else
  bad "$INSTALL_AGENT can abort at install time when the grep matches nothing (set -euo pipefail)"
fi

# 5e. The two defects driving it on a real Rocky 9 box exposed (s271).
#
# `get.docker.com` sends the RHEL clones to download.docker.com/linux/rocky,
# which carries no docker-ce — s264 fixed that in setup.sh for v2.37.0 and the
# fix never reached THIS installer, so adding a remote RHEL server had never
# once worked. Pin the el-clone repo by the baseurl both installers must use.
if grep -qF 'download.docker.com/linux/centos' "$INSTALL_AGENT"; then
  ok "$INSTALL_AGENT uses the el-clone Docker repo on the RHEL family"
else
  bad "$INSTALL_AGENT is back to get.docker.com for RHEL — linux/rocky has no docker-ce, so the install dies at step 2"
fi

# One clause, not two: the first draft OR-ed a window match against the message
# and stayed green with the message deleted, because the window found an
# unrelated `exit 1`. An assertion satisfiable two ways tests neither.
if grep -q 'Error: Docker could not be installed' "$INSTALL_AGENT"; then
  ok "$INSTALL_AGENT says so when Docker cannot be installed"
else
  bad "$INSTALL_AGENT fails silently when Docker is missing — every stream in that step goes to /dev/null"
fi

# A binary older than the flag does NOT reject it: it ignores every argument and
# starts the DAEMON, so an unbounded call hangs the installer for ever instead
# of refusing. Measured on a real box before the bound existed.
for f in "$INSTALL_AGENT" "$SELF_UPDATE"; do
  if grep -qE 'timeout [0-9]+ .*--print-unit' "$f"; then
    ok "$f bounds --print-unit, so an older binary refuses instead of hanging"
  else
    bad "$f calls --print-unit unbounded — against a pre-s271 binary that starts the daemon and never returns"
  fi
done

# And the same hazard closed at the source, for every version after this one.
if grep -qE 'unknown option' "$AGENT_MAIN"; then
  ok "the agent rejects an unknown option instead of starting the daemon"
else
  bad "an unrecognised flag once again falls through to the daemon, which binds the agent socket"
fi

echo
echo "── 6. the unit reaches boxes that ALREADY exist (lesson #97a) ──"

# The whole point. A corrected installer reaches only NEW servers; the binary is
# the one thing that reaches the fleet. Anchored on the declaration's colon —
# `has 'const RSPAMD_MILTER'` stayed green when the constant was renamed
# RSPAMD_MILTER_X (s270), and a substring pin is worth nothing.
if grep -qE 'AGENT_UNIT: &str = include_str!' "$AGENT_UNIT_RS"; then
  ok "the agent compiles the unit in from the same file the installers copy"
else
  bad "the agent no longer embeds the unit — install-agent.sh has nothing to ask for"
fi

if grep -qF 'include_str!("../../dockpanel-agent.service")' "$AGENT_UNIT_RS"; then
  ok "it embeds THAT unit, not a copy kept beside it"
else
  bad "the embedded unit no longer points at panel/agent/dockpanel-agent.service — a fourth mirror"
fi

if grep -qE 'agent_unit::heal_agent_unit\(\)' "$AGENT_MAIN"; then
  ok "the heal is spawned at agent startup"
else
  bad "heal_agent_unit is never called — an existing fleet member never receives the unit"
fi

# A heal that writes the unit but leaves the directories missing turns a silent
# security gap into a dead agent. Pin the ordering as a property of the source.
HEAL_MK=$(grep -n 'create_dir_all' "$AGENT_UNIT_RS" | head -1 | cut -d: -f1)
HEAL_WR=$(grep -n 'fs::write(UNIT_PATH' "$AGENT_UNIT_RS" | head -1 | cut -d: -f1)
if [ -n "$HEAL_MK" ] && [ -n "$HEAL_WR" ] && [ "$HEAL_MK" -lt "$HEAL_WR" ]; then
  ok "the heal creates the directories (line $HEAL_MK) before writing the unit (line $HEAL_WR)"
else
  bad "the heal writes the unit before creating its directories — the next start fails the namespace mount"
fi

if grep -qF 'LoadError' "$AGENT_UNIT_RS"; then
  ok "the heal asks systemd whether it accepted the unit, rather than assuming"
else
  bad "the heal does not check LoadError — a rejected unit would be left in place"
fi

# 6b. The self-update applies it at the SAME restart, and can put it back.
if grep -qF -- '--print-unit >' "$SELF_UPDATE"; then
  ok "$SELF_UPDATE installs the unit from the new binary"
else
  bad "$SELF_UPDATE no longer refreshes the unit — the sandbox waits for some later restart"
fi

SU_UNIT=$(grep -n 'install -m 644 "\$WORK/unit"' "$SELF_UPDATE" | head -1 | cut -d: -f1)
SU_RESTART=$(grep -n '^systemctl restart dockpanel-agent' "$SELF_UPDATE" | head -1 | cut -d: -f1)
if [ -n "$SU_UNIT" ] && [ -n "$SU_RESTART" ] && [ "$SU_UNIT" -lt "$SU_RESTART" ]; then
  ok "$SELF_UPDATE swaps the unit (line $SU_UNIT) before the restart (line $SU_RESTART) that applies it"
else
  bad "$SELF_UPDATE's unit swap no longer precedes the restart — the new sandbox would not take effect"
fi

# `grep -qF UNIT_BACKUP` matched UNIT_BACKUP_UNUSED and stayed green with the
# rollback gutted — the same substring trap as s270's `const RSPAMD_MILTER`.
# Anchored on the variable as DEREFERENCED, closing quote included.
if grep -A 30 'stage="rollback"' "$SELF_UPDATE" | grep -qF '"$UNIT_BACKUP"'; then
  ok "$SELF_UPDATE restores the previous unit on rollback"
else
  bad "$SELF_UPDATE rolls back the binary but not the unit — the old binary would start under the same bad unit"
fi

# 6c. One unit, two roles. Without the '-' the panel box refuses to start;
# without the line the fleet member loses its token and never phones home.
if grep -qF 'EnvironmentFile=-/etc/dockpanel/agent.env' "$UNIT"; then
  ok "the unit reads agent.env optionally, so it serves a panel box and a fleet member"
else
  bad "the unit's EnvironmentFile is missing or no longer optional — one of the two roles cannot start"
fi

echo
echo "── 7. every copy, DISCOVERED rather than named (s275) ──"

# Sections 2-6 pin setup.sh, update.sh, install-agent.sh and agent-self-update.sh
# BY NAME. That is the defect this section exists to close, and it has now bitten
# twice in the same suite: §5 was added at s271 because install-agent.sh — the
# third copy — was never named here and hand-wrote an unsandboxed unit for
# eighteen releases. Then s274 found a FOURTH copy, deploy-demo.sh, which had
# been unable to run since v2.37.0 for exactly the reason §3 exists: it never
# stripped the '-' prefix, so it ran `mkdir -p -/etc/letsencrypt` and died. Eight
# releases. It was invisible to every mechanism here because it lived OUTSIDE the
# repository, and a named list cannot name a file that is not in the tree.
#
# Naming the copies is what keeps failing. So this section DISCOVERS them: any
# script that mentions ReadWritePaths is a copy, and joins these assertions by
# existing. A fifth one is covered the day it is written.
#
# (agent-self-update.sh is the proof this is not theoretical — it strips the
# prefix correctly and was never named in sections 2-5 either. It was right by
# luck and unwatched by design.)

# These arms grep for shapes that this very suite, and the scripts it reads,
# DESCRIBE IN PROSE — the header of deploy-demo.sh spells `mkdir -p -/etc/…`
# because that is the bug it exists to explain. A pin that reads raw source
# matches the explanation and fires on the comment, which is how a pin earns a
# reputation for false positives and then gets deleted. So look at CODE.
#
# Full-line comments only: stripping everything after a '#' would also blank the
# inside of strings, and a check that removes more than it should fails in the
# direction that passes. A trailing comment can still fool these arms; the
# convention in this tree is that explanation goes on its own line.
code_of() { grep -v '^[[:space:]]*#' "$1"; }

# Counted, never `grep -q`, and the reason is a trap this suite walked straight
# into. Under `set -o pipefail`, `code_of f | grep -q PATTERN` reports FAILURE on
# a successful match: grep -q exits at the first hit, the producer upstream dies
# of SIGPIPE (141), and pipefail takes the pipeline's status from it. The effect
# is silent and selective — a file whose match is near the top gets dropped while
# one whose match is near the bottom survives, because the producer had already
# finished. The first run of this section discovered three of five copies that
# way and every per-file assertion below it still read green.
#
# `grep -c` consumes all of its input, so there is no early exit and no signal.
code_count() { local f="$1"; shift; grep -v '^[[:space:]]*#' "$f" | grep -c "$@"; }

RWP_SCRIPTS=$(for f in scripts/*.sh; do
  [ "$(code_count "$f" -e 'ReadWritePaths')" -gt 0 ] && echo "$f"
done | sort || true)
RWP_COUNT=$(printf '%s\n' "$RWP_SCRIPTS" | grep -c . || true)

# An arm that discovers nothing must FAIL, not pass vacuously — the s272/s273
# lesson. A glob that silently matched no files would otherwise report a clean
# suite while checking zero copies.
if [ "$RWP_COUNT" -ge 4 ]; then
  ok "discovered $RWP_COUNT scripts that touch ReadWritePaths: $(printf '%s' "$RWP_SCRIPTS" | tr '\n' ' ')"
else
  bad "discovery found only $RWP_COUNT scripts touching ReadWritePaths — four are known to exist, so this arm has gone blind rather than the copies having gone away"
fi

# 7a. Every discovered copy strips the prefix. This is the assertion that would
# have caught deploy-demo.sh in v2.37.0 instead of eight releases later.
for f in $RWP_SCRIPTS; do
  if [ "$(code_count "$f" -F -e "sed 's/^-//'")" -gt 0 ]; then
    ok "$f strips systemd's optional-path '-' prefix"
  else
    bad "$f mentions ReadWritePaths but never strips the '-' prefix — the shape that made deploy-demo.sh unrunnable for eight releases"
  fi
done

# 7b. And none of them can pass a still-prefixed path to mkdir. Catches the case
# where a script strips in one place and forgets in another.
for f in $RWP_SCRIPTS; do
  if [ "$(code_count "$f" -E -e 'mkdir[^|]*-/')" -gt 0 ]; then
    bad "$f passes a '-'-prefixed path to mkdir, which reads it as options"
  else
    ok "$f never hands mkdir a '-'-prefixed path"
  fi
done

# 7c. The copies that use the shared helper must use the SAME helper. Comparing
# the function bodies byte for byte is what makes "derivation" mean one
# derivation rather than three that happen to agree today.
HELPER_FILES=$(grep -l 'agent_rwp_dirs()' scripts/*.sh 2>/dev/null | sort || true)
HELPER_N=$(printf '%s\n' "$HELPER_FILES" | grep -c . || true)
if [ "$HELPER_N" -ge 3 ]; then
  ok "$HELPER_N scripts define agent_rwp_dirs"
else
  bad "only $HELPER_N scripts define agent_rwp_dirs — setup.sh, update.sh and deploy-demo.sh all should"
fi

REF_SUM=""; HELPER_DRIFT=0
for f in $HELPER_FILES; do
  s=$(sed -n '/^agent_rwp_dirs() {/,/^}/p' "$f" | sha256sum | cut -d' ' -f1)
  if [ -z "$REF_SUM" ]; then
    REF_SUM="$s"
  elif [ "$s" != "$REF_SUM" ]; then
    bad "$f's agent_rwp_dirs body differs from the others — the three copies have started to drift apart again"
    HELPER_DRIFT=1
  fi
done
if [ "$HELPER_DRIFT" -eq 0 ] && [ -n "$REF_SUM" ]; then
  ok "all $HELPER_N agent_rwp_dirs bodies are byte-identical"
fi

# 7d. The copy that started this is IN THE TREE. Pinned by name deliberately:
# this one assertion is about the file's location, which is the property that
# made it unreachable, and a discovery-based check cannot assert that something
# it cannot see ought to be visible.
if [ -f scripts/deploy-demo.sh ]; then
  ok "deploy-demo.sh lives in scripts/, where a pin can reach it"
else
  bad "scripts/deploy-demo.sh is gone — if the demo deploy moved back out of the repo it is unpinnable again, which is how it rotted for eight releases"
fi

# It must not carry a hostname or a checkout path of its own: the moment it does,
# it is describing one box rather than the contract, and the box-specific wrapper
# is the thing that belongs outside the tree.
if grep -qE '/home/[a-z]|[a-z0-9-]+\.dockpanel\.dev' scripts/deploy-demo.sh; then
  bad "scripts/deploy-demo.sh hardcodes a home directory or a panel hostname — those belong in the caller, not in the repo"
else
  ok "scripts/deploy-demo.sh hardcodes no path or hostname of its own"
fi

echo
echo "── 8. install-agent.sh reaches the frontend dist, on EVERY path (s274) ──"

# The panel's Add-Server dialog prints `curl -sSL {panel}/install-agent.sh |
# sudo bash`. nginx serves the SPA with a try_files fallback, so when that file
# is absent from the frontend dist the URL answers HTTP 200 WITH index.html and
# the operator pipes a web page into bash. Not a 404 — a 200 of SPA fallback
# HTML, issue #56's exact shape.
#
# setup.sh and update.sh both deploy it. deploy-demo.sh did not, for the whole
# time it existed, and the demo's own Add-Server dialog was handing out a command
# that returned 643 bytes of <!DOCTYPE html>. Found at s274 by fetching it the
# way an operator does.
#
# Same discovery rule as §7: any script that populates the frontend dist owes
# this step, and joins the check by existing.
# Discovery is on the DIST-ROOT ASSIGNMENT, not on a literal path. The three
# scripts spell the same directory three different ways —
#   setup.sh        FE_ROOT="${FRONTEND_DIR}/dist"
#   update.sh       FE_DIST="${REPO_DIR}/panel/frontend/dist"
#   deploy-demo.sh  FE_DIST="$REPO_DIR/panel/frontend/dist"
# — so the first draft of this arm, matching `frontend/dist`, found two of three
# and MISSED setup.sh, the one installer every user runs. It said so instead of
# passing, which is the only reason the gap was visible.
FE_SCRIPTS=$(for f in scripts/*.sh; do
  [ "$(code_count "$f" -Ei -e '(frontend|fe_dir|fe_root|fe_dist)[^=]*=.*/dist')" -gt 0 ] && echo "$f"
done | sort || true)
FE_COUNT=$(printf '%s\n' "$FE_SCRIPTS" | grep -c . || true)

if [ "$FE_COUNT" -ge 3 ]; then
  ok "discovered $FE_COUNT scripts that populate the frontend dist: $(printf '%s' "$FE_SCRIPTS" | tr '\n' ' ')"
else
  bad "discovery found only $FE_COUNT scripts touching the frontend dist — setup.sh, update.sh and deploy-demo.sh all do, so this arm has gone blind"
fi

for f in $FE_SCRIPTS; do
  # Anchored on the COPY ITSELF — a `cp` or `install` command whose destination
  # is install-agent.sh — not on the filename appearing somewhere in the script.
  #
  # The first draft asked only whether the name appeared in code, and every one
  # of these scripts also ANNOUNCES the step (`echo "=== drop install-agent.sh
  # …"`, `log "Refreshed install-agent.sh in $FE_DIST"`). Deleting the actual
  # copy and leaving the announcement kept the pin green — the check passed on a
  # script that had stopped doing the thing while still saying it did. Found by
  # mutating it. A presence check is not an operation check (#100e).
  if [ "$(code_count "$f" -E -e '^[[:space:]]*(cp|install)[[:space:]].*install-agent\.sh')" -gt 0 ]; then
    ok "$f copies install-agent.sh into the frontend dist"
  else
    bad "$f populates the frontend dist but performs no copy of install-agent.sh — {panel}/install-agent.sh will answer 200 with SPA HTML, and the Add-Server command pipes that into bash"
  fi
done

# The source it copies must exist. A step that copies a file that was renamed
# away is a step that silently does nothing.
if [ -f scripts/install-agent.sh ]; then
  ok "scripts/install-agent.sh exists, so those copy steps have a source"
else
  bad "scripts/install-agent.sh is missing — every deploy step above copies nothing and the URL falls back to the SPA"
fi

echo
echo "── 9. the BUILD re-emits it, because the build is what deletes it (s275) ──"

# The root cause underneath section 8, found by the live-surfaces arm added in
# the same release, minutes after it was deployed.
#
# install-agent.sh lived ONLY in panel/frontend/dist. Vite empties outDir on
# every build, so `npm run build` — an ordinary command with nothing to do with
# the fleet — DELETED the installer from the live panel, and the Add-Server
# command served 200 with SPA fallback HTML until some installer happened to run
# again. s274 read this as "deploy-demo.sh forgot a step" and fixed that one
# path. The real defect is that the artefact only ever lived in a directory a
# routine command wipes.
#
# The marketing site had already solved it: website/client/public/install.sh is
# copied into dist by every build. The panel now does the same, staged from the
# single source in scripts/ rather than a second committed copy.
FE_PKG=panel/frontend/package.json
STAGER=panel/frontend/scripts/stage-install-agent.mjs

if [ -f "$STAGER" ]; then
  ok "the frontend has a stage-install-agent step"
else
  bad "$STAGER is gone — a frontend rebuild will empty dist/ and silently un-deploy the Add-Server installer"
fi

# Wired to the BUILD, not merely present. An unreferenced script is not a step.
if grep -qF 'stage-install-agent' "$FE_PKG"; then
  ok "the frontend build invokes it (prebuild)"
else
  bad "$FE_PKG does not run the stager — the script exists but no build calls it, so dist/ loses the installer again"
fi

# It must read the SINGLE source. A stager copying from a checked-in duplicate
# would make a fifth copy of an installer, which is the mistake this tree keeps
# repeating (four copies of the ReadWritePaths derivation, three of the unit).
if [ -f "$STAGER" ] && grep -qF 'scripts/install-agent.sh' "$STAGER"; then
  ok "it stages from scripts/install-agent.sh, the single source"
else
  bad "the stager no longer reads scripts/install-agent.sh — it is copying from somewhere else, which means a second copy exists"
fi

# And it must FAIL when the source is absent rather than skipping. A silent skip
# reproduces the defect exactly: build succeeds, panel serves HTML, operator
# pipes a web page into sudo bash.
if [ -f "$STAGER" ] && grep -qE 'process\.exit\(1\)' "$STAGER"; then
  ok "it exits non-zero when the source is missing, rather than skipping"
else
  bad "the stager does not fail on a missing source — a silent skip ships a panel whose Add-Server URL returns SPA HTML"
fi

# The staged copy is a build artefact. If it ever gets committed it becomes a
# second source that can drift from scripts/install-agent.sh.
if grep -qF 'panel/frontend/public/install-agent.sh' .gitignore; then
  ok "the staged copy is gitignored, so it cannot become a second committed source"
else
  bad "panel/frontend/public/install-agent.sh is not gitignored — it will be committed and become a copy that drifts"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
