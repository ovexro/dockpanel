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
#
# ─────────────────────────────────────────────────────────────────────────────
# s337 — WHAT THIS SUITE MEASURED, AND WHAT IT ONLY APPEARED TO
#
# Sections 7 and 8 have read comment-stripped source since s275. Sections 1-6
# did not: they greped the RAW file. The strippers were defined at line 379,
# INSIDE section 7, and every call site was below the definition — so the
# thirty-odd arms above it were reading prose as if it were code, and section 7's
# own header explained the hazard and then stopped one section short.
#
# The consequence, measured at 918120a by mutation rather than argued:
#
#   TWENTY-THREE of the 64 assertions were satisfiable by a COMMENT, with the
#   suite still reporting `passed: 64  failed: 0`.
#
# The worst case is the one this suite exists for. Section 5 was added at s271
# because install-agent.sh hand-wrote its own unsandboxed unit for eighteen
# releases. Replay that defect in its exact shape — delete the `--print-unit`
# call, leave the explanatory comment behind, and write a unit body with a
# tab-indented heredoc — and ALL FIVE arms guarding it stayed green: four were
# reading the comment, and `^\[Service\]` was anchored at column 0 so an indented
# body slipped under it. The suite passed 64/0 on the very regression it was
# written to catch.
#
# Three arms fail in a nastier way still, because they compare `grep -n` LINE
# NUMBERS. Physically MOVING update.sh's mkdir loop to below the unit install and
# leaving a comment at the old position kept the arm green — and it printed
# "creates the directories (line 465) before installing the unit (line 563)",
# quoting the comment's line number as evidence for an ordering that no longer
# held. Same for the heal in agent_unit.rs and the swap in agent-self-update.sh.
#
# So: the strippers now sit ABOVE section 1, they understand the comment syntax
# of each subject language rather than assuming `#`, and they BLANK comment lines
# instead of deleting them — deleting renumbers the file, which is precisely what
# the three ordering arms measure. Sections 10-12 hold all of that in place, and
# section 11 is the mutation itself, encoded so it runs on every push rather than
# living in one session's notes.
# ─────────────────────────────────────────────────────────────────────────────

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# ── the comment strippers, ABOVE every arm that needs them (s337) ────────────
#
# A check placed below the thing it must run before inherits that position as a
# defect even when the check itself is perfect — the s336 lesson, and the reason
# these three functions are the first thing in the file rather than the middle.
#
# The comment marker is chosen by EXTENSION, never guessed. Getting it wrong is
# not symmetric: over-stripping turns a positive arm loudly red, but turns a
# NEGATIVE arm (match ⇒ bad) silently green, which is the direction that ships.
comment_marker_for() {
  case "$1" in
    *.rs|*.mjs|*.js|*.ts|*.tsx)                echo '//' ;;
    *.json)                                    echo ''   ;;  # JSON has no comments
    *)                                         echo '#'  ;;  # .sh, .service, .gitignore, .toml
  esac
}

# BLANK the comment, do not delete it. Three arms below compare line numbers
# obtained from this stream against each other; a stripper that removes lines
# renumbers the file and would silently change what those arms measure.
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
    # double quotes — otherwise `let u = "https://host"` would lose its tail.
    # Backslash escapes are skipped so `\"` does not flip the parity.
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
    # Shell keeps to FULL-LINE comments only, and the asymmetry with `//` above is
    # deliberate. A trailing `#` cannot be stripped safely here: `${VAR#prefix}`,
    # `$#`, a colour literal and `sed 's/#//'` are all ordinary code. The
    # convention in this tree is that explanation goes on its own line, and §10
    # asserts the stripper leaves a `#` inside a string alone.
    awk '/^[[:space:]]*#/ { print ""; next } { print }' "$f"
  fi
}

# Counted, never `grep -q`, and the reason is a trap this suite walked straight
# into twice. Under `set -o pipefail`, `code_lines f | grep -q PATTERN` reports
# FAILURE on a successful match: grep -q exits at the first hit, the producer
# upstream dies of SIGPIPE (141), and pipefail takes the pipeline's status from
# it. The effect is silent and selective — a file whose match is near the top gets
# dropped while one whose match is near the bottom survives, because the producer
# had already finished. `grep -c` consumes all of its input, so there is no early
# exit and no signal. (s336 #365 is the same operator lying in the other
# direction: a failed PRODUCER read as a clean no-match.)
code_count() { local f="$1"; shift; code_lines "$f" | grep -c "$@" 2>/dev/null || true; }
has_code()   { local f="$1"; shift; [ "$(code_count "$f" "$@")" -gt 0 ]; }

# True file line numbers, because code_lines blanks rather than deletes.
code_lineno() { local f="$1"; shift; code_lines "$f" | grep -n "$@" 2>/dev/null | head -1 | cut -d: -f1 || true; }

UNIT=panel/agent/dockpanel-agent.service
SETUP=scripts/setup.sh
UPDATE=scripts/update.sh

for f in "$UNIT" "$SETUP" "$UPDATE"; do
  [ -f "$f" ] || { echo "missing file: $f"; exit 1; }
done

# The unit is the single source of truth. Read it the same way the installers do.
RWP_LINE=$(code_lines "$UNIT" | grep '^ReadWritePaths=' | head -1)
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

if has_code "$UNIT" -E -e '^ProtectSystem=strict'; then
  ok "ProtectSystem=strict — an unbound path really is read-only"
else
  bad "ProtectSystem is no longer strict; the sandbox these paths carve out may not exist"
fi

echo
echo "── 2. both installers DERIVE the list rather than mirroring it ──"

for f in "$SETUP" "$UPDATE"; do
  if has_code "$f" -F -e 'agent_rwp_dirs()'; then
    ok "$f defines agent_rwp_dirs"
  else
    bad "$f no longer derives the directory list from the unit — a hand-written list is how this drifted"
  fi
  # Defining it and never calling it would pass a naive pin while restoring the bug.
  if [ "$(code_count "$f" -F -e 'agent_rwp_dirs')" -ge 2 ]; then
    ok "$f actually calls it"
  else
    bad "$f defines agent_rwp_dirs but never calls it"
  fi
done

# Under `set -euo pipefail` a grep that matches nothing kills the script, and
# these run at install time where the failure is worst.
for f in "$SETUP" "$UPDATE"; do
  if [ "$(code_lines "$f" | grep -A 6 -F -e 'agent_rwp_dirs()' | grep -cF '|| true' || true)" -gt 0 ]; then
    ok "$f's helper tolerates a no-match under pipefail"
  else
    bad "$f's helper can abort the installer when the grep matches nothing (set -euo pipefail)"
  fi
done

echo
echo "── 3. the derivation reproduces the unit exactly ──"

# Run the real helper, as defined in the shipped script, against the real unit.
# This one reads the RAW file on purpose: it EXECUTES what it extracts, so a
# commented-out helper yields nothing, `agent_rwp_dirs` is undefined, and the arm
# goes red of its own accord. Execution is its own comment stripper.
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
if printf '%s\n' "$DERIVED" | grep -c '^-' >/dev/null; then
  bad "derived paths still carry the '-' prefix; mkdir would treat it as an option"
else
  ok "the '-' prefix is stripped from every derived path"
fi

echo
echo "── 4. the specific drift that started this, and the ordering that makes it work ──"

# /var/spool/cron is the path that drifted. Pinning it by name keeps the actual
# regression down even if the derivation is rewritten later.
for f in "$SETUP" "$UPDATE"; do
  if printf '%s\n' "$DERIVED" | grep -cx '/var/spool/cron' >/dev/null; then
    ok "$f covers /var/spool/cron (via the derivation)"
  else
    bad "$f does not cover /var/spool/cron — the s268 cron fix stops landing on upgraded boxes"
  fi
done

if printf '%s\n' "$ALL" | grep -cx '/var/spool/cron' >/dev/null; then
  ok "the unit still grants the cron spool, so crontabs remain writable"
else
  bad "/var/spool/cron left the unit — the agent cannot write crontabs at all"
fi

# The directories must exist BEFORE the agent restarts, or the bind is skipped
# for the new namespace and the fix is inert until the next restart.
#
# Both line numbers come from the BLANKED stream, so they are true file lines and
# a comment cannot supply either one. Before s337 they came from the raw file:
# moving the mkdir loop below the unit install and leaving a comment behind kept
# this arm green AND made it print the comment's line number as its evidence.
MK_LINE=$(code_lineno "$UPDATE" -F -e 'for d in $(agent_rwp_dirs)')
CP_LINE=$(code_lineno "$UPDATE" -F -e 'cp "$AGENT_SRC/dockpanel-agent.service"')
if [ -n "$MK_LINE" ] && [ -n "$CP_LINE" ] && [ "$MK_LINE" -lt "$CP_LINE" ]; then
  ok "update.sh creates the directories (line $MK_LINE) before installing the unit (line $CP_LINE)"
else
  bad "update.sh's directory creation no longer precedes the unit install — the new namespace can be built before the paths exist (mkdir=${MK_LINE:-none} cp=${CP_LINE:-none})"
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
  if has_code "$INSTALL_AGENT" -E -e "^[[:space:]]*${directive}=no"; then
    bad "$INSTALL_AGENT sets ${directive}=no again — that is the shape every fleet member ran unsandboxed under"
  else
    ok "$INSTALL_AGENT does not disable ${directive}"
  fi
done

# The anchor was `^\[Service\]` — column zero — until s337. A heredoc written
# `<<-UNIT` with a tab-indented body slid straight under it, so a replay of the
# s271 defect that KEPT the four hardening directives above passed this arm too.
if has_code "$INSTALL_AGENT" -E -e '^[[:space:]]*\[Service\]'; then
  bad "$INSTALL_AGENT contains a unit body again — the third copy is back"
else
  ok "$INSTALL_AGENT contains no unit body at all"
fi

# 5b. It takes the unit from the binary it just downloaded. Anchored on the flag
# followed by the redirect, not on the flag as a substring: `--print-unit` alone
# still matches `--print-unit-x`, and a pin that survives a rename is worth
# nothing (s270's `const RSPAMD_MILTER`).
if has_code "$INSTALL_AGENT" -F -e '--print-unit >'; then
  ok "$INSTALL_AGENT obtains the unit from the agent binary"
else
  bad "$INSTALL_AGENT no longer asks the binary for the unit — it is writing one from somewhere"
fi

# The flag is a contract between three files. If main.rs stops answering to the
# exact string the installers send, install-agent.sh refuses every install and
# the self-update silently stops refreshing the unit.
if has_code "$AGENT_MAIN" -F -e '"--print-unit"'; then
  ok "the agent answers to the same flag the installers invoke"
else
  bad "main.rs no longer handles \"--print-unit\" — the installers ask for a unit nothing emits"
fi

# 5c. And REFUSES rather than falling back. A fallback here is what the whole
# defect was: a locally-invented unit that nobody compared to the real one.
if [ "$(code_lines "$INSTALL_AGENT" | grep -A 8 -F -e '--print-unit' | grep -cE '^[[:space:]]*exit 1' || true)" -gt 0 ]; then
  ok "$INSTALL_AGENT exits non-zero when the binary cannot emit a unit"
else
  bad "$INSTALL_AGENT does not fail when --print-unit fails — an absent unit must be an error, never a guess"
fi

# 5d. The derived directory list, same rule as setup.sh/update.sh in section 2.
# /etc/nginx is UNPREFIXED and an agent-only box has no nginx, so without this
# the hardened unit makes every fleet member unstartable.
if has_code "$INSTALL_AGENT" -F -e 'ReadWritePaths=' && has_code "$INSTALL_AGENT" -F -e "sed 's/^-//'"; then
  ok "$INSTALL_AGENT derives its mkdir list from the installed unit, prefix stripped"
else
  bad "$INSTALL_AGENT does not derive the writable-path directories — /etc/nginx alone would make the agent unstartable on a fleet member"
fi

# Anchored on the guarded grep ITSELF, not on `|| true` appearing anywhere in
# the next few lines — the mkdir loop below it carries its own `|| true`, so the
# window form passed with the guard deleted. Found by mutating it (#97h).
if has_code "$INSTALL_AGENT" -F -e "{ grep '^/' || true; }"; then
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
if has_code "$INSTALL_AGENT" -F -e 'download.docker.com/linux/centos'; then
  ok "$INSTALL_AGENT uses the el-clone Docker repo on the RHEL family"
else
  bad "$INSTALL_AGENT is back to get.docker.com for RHEL — linux/rocky has no docker-ce, so the install dies at step 2"
fi

# One clause, not two: the first draft OR-ed a window match against the message
# and stayed green with the message deleted, because the window found an
# unrelated `exit 1`. An assertion satisfiable two ways tests neither.
if has_code "$INSTALL_AGENT" -F -e 'Error: Docker could not be installed'; then
  ok "$INSTALL_AGENT says so when Docker cannot be installed"
else
  bad "$INSTALL_AGENT fails silently when Docker is missing — every stream in that step goes to /dev/null"
fi

# A binary older than the flag does NOT reject it: it ignores every argument and
# starts the DAEMON, so an unbounded call hangs the installer for ever instead
# of refusing. Measured on a real box before the bound existed.
for f in "$INSTALL_AGENT" "$SELF_UPDATE"; do
  if has_code "$f" -E -e 'timeout [0-9]+ .*--print-unit'; then
    ok "$f bounds --print-unit, so an older binary refuses instead of hanging"
  else
    bad "$f calls --print-unit unbounded — against a pre-s271 binary that starts the daemon and never returns"
  fi
done

# And the same hazard closed at the source, for every version after this one.
if has_code "$AGENT_MAIN" -E -e 'unknown option'; then
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
if has_code "$AGENT_UNIT_RS" -E -e 'AGENT_UNIT: &str = include_str!'; then
  ok "the agent compiles the unit in from the same file the installers copy"
else
  bad "the agent no longer embeds the unit — install-agent.sh has nothing to ask for"
fi

if has_code "$AGENT_UNIT_RS" -F -e 'include_str!("../../dockpanel-agent.service")'; then
  ok "it embeds THAT unit, not a copy kept beside it"
else
  bad "the embedded unit no longer points at panel/agent/dockpanel-agent.service — a fourth mirror"
fi

if has_code "$AGENT_MAIN" -E -e 'agent_unit::heal_agent_unit\(\)'; then
  ok "the heal is spawned at agent startup"
else
  bad "heal_agent_unit is never called — an existing fleet member never receives the unit"
fi

# A heal that writes the unit but leaves the directories missing turns a silent
# security gap into a dead agent. Pin the ordering as a property of the source.
HEAL_MK=$(code_lineno "$AGENT_UNIT_RS" -F -e 'create_dir_all')
HEAL_WR=$(code_lineno "$AGENT_UNIT_RS" -F -e 'fs::write(UNIT_PATH')
if [ -n "$HEAL_MK" ] && [ -n "$HEAL_WR" ] && [ "$HEAL_MK" -lt "$HEAL_WR" ]; then
  ok "the heal creates the directories (line $HEAL_MK) before writing the unit (line $HEAL_WR)"
else
  bad "the heal writes the unit before creating its directories — the next start fails the namespace mount (mkdir=${HEAL_MK:-none} write=${HEAL_WR:-none})"
fi

if has_code "$AGENT_UNIT_RS" -F -e 'LoadError'; then
  ok "the heal asks systemd whether it accepted the unit, rather than assuming"
else
  bad "the heal does not check LoadError — a rejected unit would be left in place"
fi

# 6b. The self-update applies it at the SAME restart, and can put it back.
if has_code "$SELF_UPDATE" -F -e '--print-unit >'; then
  ok "$SELF_UPDATE installs the unit from the new binary"
else
  bad "$SELF_UPDATE no longer refreshes the unit — the sandbox waits for some later restart"
fi

SU_UNIT=$(code_lineno "$SELF_UPDATE" -F -e 'install -m 644 "$WORK/unit"')
SU_RESTART=$(code_lineno "$SELF_UPDATE" -E -e '^systemctl restart dockpanel-agent')
if [ -n "$SU_UNIT" ] && [ -n "$SU_RESTART" ] && [ "$SU_UNIT" -lt "$SU_RESTART" ]; then
  ok "$SELF_UPDATE swaps the unit (line $SU_UNIT) before the restart (line $SU_RESTART) that applies it"
else
  bad "$SELF_UPDATE's unit swap no longer precedes the restart — the new sandbox would not take effect (swap=${SU_UNIT:-none} restart=${SU_RESTART:-none})"
fi

# `grep -qF UNIT_BACKUP` matched UNIT_BACKUP_UNUSED and stayed green with the
# rollback gutted — the same substring trap as s270's `const RSPAMD_MILTER`.
# Anchored on the variable as DEREFERENCED, closing quote included.
if [ "$(code_lines "$SELF_UPDATE" | grep -A 30 -F -e 'stage="rollback"' | grep -cF '"$UNIT_BACKUP"' || true)" -gt 0 ]; then
  ok "$SELF_UPDATE restores the previous unit on rollback"
else
  bad "$SELF_UPDATE rolls back the binary but not the unit — the old binary would start under the same bad unit"
fi

# 6c. One unit, two roles. Without the '-' the panel box refuses to start;
# without the line the fleet member loses its token and never phones home.
if has_code "$UNIT" -F -e 'EnvironmentFile=-/etc/dockpanel/agent.env'; then
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

RWP_SCRIPTS=$(for f in scripts/*.sh; do
  has_code "$f" -F -e 'ReadWritePaths' && echo "$f"
done | sort || true)
RWP_COUNT=$(printf '%s\n' "$RWP_SCRIPTS" | grep -c . || true)

# An arm that discovers nothing must FAIL, not pass vacuously — the s272/s273
# lesson. A glob that silently matched no files would otherwise report a clean
# suite while checking zero copies.
#
# The floor is a RATCHET at the true count, not a comfortable margin. It sat at
# 4 while five copies existed, so a copy could leave the population unnoticed:
# commenting out install-agent.sh's ReadWritePaths line dropped the suite to
# `passed: 62  failed: 0` — two assertions gone, exit 0, and green on screen.
# Raise it whenever a copy is legitimately added.
if [ "$RWP_COUNT" -ge 5 ]; then
  ok "discovered $RWP_COUNT scripts that touch ReadWritePaths: $(printf '%s' "$RWP_SCRIPTS" | tr '\n' ' ')"
else
  bad "discovery found only $RWP_COUNT scripts touching ReadWritePaths — five are known to exist, so this arm has gone blind rather than the copies having gone away"
fi

# 7a. Every discovered copy strips the prefix. This is the assertion that would
# have caught deploy-demo.sh in v2.37.0 instead of eight releases later.
for f in $RWP_SCRIPTS; do
  if has_code "$f" -F -e "sed 's/^-//'"; then
    ok "$f strips systemd's optional-path '-' prefix"
  else
    bad "$f mentions ReadWritePaths but never strips the '-' prefix — the shape that made deploy-demo.sh unrunnable for eight releases"
  fi
done

# 7b. And none of them can pass a still-prefixed path to mkdir. Catches the case
# where a script strips in one place and forgets in another.
for f in $RWP_SCRIPTS; do
  if has_code "$f" -E -e 'mkdir[^|]*-/'; then
    bad "$f passes a '-'-prefixed path to mkdir, which reads it as options"
  else
    ok "$f never hands mkdir a '-'-prefixed path"
  fi
done

# 7c. The copies that use the shared helper must use the SAME helper. Comparing
# the function bodies byte for byte is what makes "derivation" mean one
# derivation rather than three that happen to agree today.
HELPER_FILES=$(for f in scripts/*.sh; do
  has_code "$f" -F -e 'agent_rwp_dirs()' && echo "$f"
done | sort || true)
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
#
# Read from CODE since s337. This is a negative arm, so the failure it used to
# have was the loud kind: appending an ordinary usage comment naming the demo
# host turned it red on a correct file. §11 plants a matching hostname in code to
# prove it still fires.
if has_code scripts/deploy-demo.sh -E -e '/home/[a-z]|[a-z0-9-]+\.dockpanel\.dev'; then
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
  has_code "$f" -Ei -e '(frontend|fe_dir|fe_root|fe_dist)[^=]*=.*/dist' && echo "$f"
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
  if has_code "$f" -E -e '^[[:space:]]*(cp|install)[[:space:]].*install-agent\.sh'; then
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
# package.json is JSON, which has no comment syntax, so this one arm was never
# reachable by the class §10-§12 exist for.
if has_code "$FE_PKG" -F -e 'stage-install-agent'; then
  ok "the frontend build invokes it (prebuild)"
else
  bad "$FE_PKG does not run the stager — the script exists but no build calls it, so dist/ loses the installer again"
fi

# It must read the SINGLE source. A stager copying from a checked-in duplicate
# would make a fifth copy of an installer, which is the mistake this tree keeps
# repeating (four copies of the ReadWritePaths derivation, three of the unit).
if [ -f "$STAGER" ] && has_code "$STAGER" -F -e 'scripts/install-agent.sh'; then
  ok "it stages from scripts/install-agent.sh, the single source"
else
  bad "the stager no longer reads scripts/install-agent.sh — it is copying from somewhere else, which means a second copy exists"
fi

# And it must FAIL when the source is absent rather than skipping. A silent skip
# reproduces the defect exactly: build succeeds, panel serves HTML, operator
# pipes a web page into sudo bash.
if [ -f "$STAGER" ] && has_code "$STAGER" -E -e 'process\.exit\(1\)'; then
  ok "it exits non-zero when the source is missing, rather than skipping"
else
  bad "the stager does not fail on a missing source — a silent skip ships a panel whose Add-Server URL returns SPA HTML"
fi

# The staged copy is a build artefact. If it ever gets committed it becomes a
# second source that can drift from scripts/install-agent.sh.
if has_code .gitignore -F -e 'panel/frontend/public/install-agent.sh'; then
  ok "the staged copy is gitignored, so it cannot become a second committed source"
else
  bad "panel/frontend/public/install-agent.sh is not gitignored — it will be committed and become a copy that drifts"
fi

echo
echo "── 10. the strippers strip comments, and nothing else (s337) ──"

# Everything above now depends on code_lines. A stripper that under-strips
# restores the whole defect silently; one that OVER-strips turns the negative
# arms in §5 and §7d green on broken code. So it gets its own fixtures, and they
# are hermetic — no repository file is involved, because a fixture that shares a
# subject with the thing it validates cannot tell the two apart.
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

# The three ordering arms read line numbers out of this stream. Deleting rather
# than blanking would renumber every subject and change what they measure.
LINEDRIFT=""
for f in "$UNIT" "$SETUP" "$UPDATE" "$INSTALL_AGENT" "$SELF_UPDATE" "$AGENT_UNIT_RS" "$AGENT_MAIN" "$STAGER" scripts/deploy-demo.sh .gitignore; do
  a=$(wc -l < "$f"); b=$(code_lines "$f" | wc -l)
  [ "$a" != "$b" ] && LINEDRIFT="$LINEDRIFT $f($a≠$b)"
done
if [ -z "$LINEDRIFT" ]; then
  ok "blanking preserves the line count of all 10 subjects, so line numbers stay true"
else
  bad "code_lines changed the line count of:$LINEDRIFT — the ordering arms are now measuring the wrong lines"
fi

# The most direct over-strip control available: if the stripper removed part of a
# command, the result stops being a valid script.
PARSEFAIL=""
for f in "$SETUP" "$UPDATE" "$INSTALL_AGENT" "$SELF_UPDATE" scripts/deploy-demo.sh; do
  code_lines "$f" | bash -n 2>/dev/null || PARSEFAIL="$PARSEFAIL $f"
done
if [ -z "$PARSEFAIL" ]; then
  ok "every shell subject still parses after blanking, so no command was cut in half"
else
  bad "these no longer parse after blanking, so the stripper removed code:$PARSEFAIL"
fi

echo
echo "── 11. every pinned pattern is live CODE, and a comment cannot satisfy it (s337) ──"

# This is the s337 mutation, encoded. A round of mutation testing done by hand
# dies with the session that did it; the arms below re-run it on every push.
#
# For each subject: assert every pattern this suite pins is present in CODE (a
# pattern that has quietly become prose-only would otherwise turn red later and
# look like a test bug rather than the defect it is), then turn each matching
# code line into a comment in a throwaway copy and assert the count falls to
# zero. If it does not, some arm above is still reading the file raw.
PINNED=(
  "$SETUP|-F|agent_rwp_dirs()"
  "$UPDATE|-F|agent_rwp_dirs()"
  "$UPDATE|-F|for d in \$(agent_rwp_dirs)"
  "$UPDATE|-F|cp \"\$AGENT_SRC/dockpanel-agent.service\""
  "$INSTALL_AGENT|-F|--print-unit >"
  "$INSTALL_AGENT|-F|sed 's/^-//'"
  "$INSTALL_AGENT|-F|ReadWritePaths="
  "$INSTALL_AGENT|-F|{ grep '^/' || true; }"
  "$INSTALL_AGENT|-F|download.docker.com/linux/centos"
  "$INSTALL_AGENT|-F|Error: Docker could not be installed"
  "$INSTALL_AGENT|-E|timeout [0-9]+ .*--print-unit"
  "$SELF_UPDATE|-F|--print-unit >"
  "$SELF_UPDATE|-E|timeout [0-9]+ .*--print-unit"
  "$SELF_UPDATE|-F|install -m 644 \"\$WORK/unit\""
  "$SELF_UPDATE|-E|^systemctl restart dockpanel-agent"
  "$SELF_UPDATE|-F|stage=\"rollback\""
  "$SELF_UPDATE|-F|\"\$UNIT_BACKUP\""
  "$AGENT_MAIN|-F|\"--print-unit\""
  "$AGENT_MAIN|-E|unknown option"
  "$AGENT_MAIN|-E|agent_unit::heal_agent_unit\\(\\)"
  "$AGENT_UNIT_RS|-E|AGENT_UNIT: &str = include_str!"
  "$AGENT_UNIT_RS|-F|include_str!(\"../../dockpanel-agent.service\")"
  "$AGENT_UNIT_RS|-F|LoadError"
  "$AGENT_UNIT_RS|-F|create_dir_all"
  "$AGENT_UNIT_RS|-F|fs::write(UNIT_PATH"
  "$UNIT|-F|EnvironmentFile=-/etc/dockpanel/agent.env"
  "$UNIT|-E|^ReadWritePaths="
  "$UNIT|-E|^ProtectSystem=strict"
  "$STAGER|-F|scripts/install-agent.sh"
  "$STAGER|-E|process\\.exit\\(1\\)"
  ".gitignore|-F|panel/frontend/public/install-agent.sh"
)

# Turn every CODE line matching the pattern into a comment, leaving the text.
# That is the defect this section is about: the mechanism removed, the
# explanation left behind.
#
# The copy KEEPS the original's basename, and that is load-bearing rather than
# tidy: comment_marker_for dispatches on the extension, so a scratch file called
# `mutated` is read as shell no matter what it holds. The first draft did exactly
# that and the three `//` subjects reported the stripper broken when what was
# broken was the temp filename — the arm measured the harness, not the subject.
comment_out_matches() {
  local src="$1" dst="$2" mode="$3" pat="$4" marker
  marker=$(comment_marker_for "$src")
  code_lines "$src" | grep -n "$mode" -e "$pat" 2>/dev/null | cut -d: -f1 > "$FIXDIR/hits" || true
  awk -v marker="$marker" 'NR==FNR { hit[$1]=1; next } { if (FNR in hit) print marker " " $0; else print }' \
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

# The negative arms are narrowed by the stripper, and a narrowing fails in the
# silent direction: a stripper that removes too much would make these pass on
# genuinely broken files. So plant the real defect, in code, and require a fire.
cp "$INSTALL_AGENT" "$FIXDIR/ia.sh"
printf '\n    NoNewPrivileges=no\n' >> "$FIXDIR/ia.sh"
if has_code "$FIXDIR/ia.sh" -E -e '^[[:space:]]*NoNewPrivileges=no'; then
  ok "§5's sandbox-directive arm still fires when the directive is really in code"
else
  bad "§5's sandbox-directive arm no longer fires on a real NoNewPrivileges=no — the stripper is eating code"
fi

# A host that MATCHES the arm's pattern without being a real one. Spelling the
# actual demo hostname here would put an internal address in the tree for no
# gain, and the pre-commit leak scan is right to complain about it.
cp scripts/deploy-demo.sh "$FIXDIR/dd.sh"
printf '\nDEMO_HOST=example.dockpanel.dev\n' >> "$FIXDIR/dd.sh"
if has_code "$FIXDIR/dd.sh" -E -e '/home/[a-z]|[a-z0-9-]+\.dockpanel\.dev'; then
  ok "§7d's hardcoded-hostname arm still fires when a hostname is really in code"
else
  bad "§7d's hardcoded-hostname arm no longer fires on a real hostname — the stripper is eating code"
fi

echo
echo "── 12. no arm in this suite may read a subject raw again (s337) ──"

# The structural guard. §11 proves today's arms are comment-blind; this one
# stops a NEW arm from being written the old way, which is exactly how sections
# 1-6 came to differ from 7-8 in the first place.
SELF=tests/sandbox-paths-pin-e2e.sh

# Assembled at runtime from a variable, so this line does not contain the shape
# it searches for and the scan cannot match itself (#366 — the bracket idiom's
# purpose, reached here without a path exemption).
SUBJ_VARS='UNIT|SETUP|UPDATE|INSTALL_AGENT|SELF_UPDATE|AGENT_UNIT_RS|AGENT_MAIN|STAGER'
RAW_ARMS=$(code_lines "$SELF" | grep -nE "grep[^|;]*\"\\\$($SUBJ_VARS)\"" || true)
if [ -z "$RAW_ARMS" ]; then
  ok "no arm greps a subject file directly; every read goes through code_lines"
else
  bad "these lines read a subject raw instead of through code_lines: $(printf '%s' "$RAW_ARMS" | cut -d: -f1 | tr '\n' ' ')"
fi

# s336 #364: a check placed below an early exit inherits that exit's blind spot.
# The strippers were the same shape — defined at line 379, used by nothing above
# it. Assert the definition precedes the first section rather than trusting it.
DEF_LINE=$(code_lineno "$SELF" -E -e '^code_lines\(\) \{')
SEC1_LINE=$(code_lineno "$SELF" -F -e '── 1. the unit still declares')
if [ -n "$DEF_LINE" ] && [ -n "$SEC1_LINE" ] && [ "$DEF_LINE" -lt "$SEC1_LINE" ]; then
  ok "code_lines is defined (line $DEF_LINE) above the first section (line $SEC1_LINE)"
else
  bad "code_lines is no longer defined above section 1 — the arms above its definition are reading raw source again (def=${DEF_LINE:-none} §1=${SEC1_LINE:-none})"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
