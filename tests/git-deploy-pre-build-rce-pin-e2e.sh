#!/usr/bin/env bash
# Regression pins for the Git Deploy pre-build-hook root-RCE fix (v2.182.0).
#
# panel/agent/src/routes/git_build.rs's pre_build_hook (POST /git/pre-build-hook)
# ran a whitelisted-by-literal-string command via `sh -c` directly on the host,
# as root (dockpanel-agent.service carries no User= directive). The whitelist
# blocked shell-injection into the command STRING but did nothing about the
# named commands themselves being standard supply-chain-RCE vectors — every
# one of npm/yarn/pnpm/pip/composer/bundle/cargo runs installer/build hooks
# (package.json postinstall, setup.py, build.rs, ...) that execute arbitrary
# code. An admin arming any one of them for a git deploy caused every
# subsequent repo push AND every transitive dependency to run as root on the
# panel host, unattended. First identified s244, re-verified open at s428
# after 50 sessions during which an audit-coverage blind spot (backend-only
# candidate enumeration, see feedback_dockpanel_audit_scope) kept it off every
# rotation pick.
#
# THE FIX: delete the host-exec route entirely. The same whitelisted string is
# now spliced into the auto-generated Dockerfile's install RUN line
# (auto_generate_dockerfile), so it only ever executes inside the `docker
# build` sandbox every other git deploy on this platform already trusts —
# never a host shell. A design panel scored this against three sandboxing
# alternatives (ephemeral container, privilege-drop via systemd-run, a
# bubblewrap namespace) and it won on containment, fleet-compat, and
# architecture-fit simultaneously: it doesn't add a new isolation boundary
# around the vulnerable execution, it deletes the vulnerable execution path.
#
# Bonus, independently verified: the old code ran pre_build_hook
# UNCONDITIONALLY even after a successful Nixpacks build, silently discarding
# its host-side mutation. The new code skips Nixpacks entirely whenever a
# pre-build command is configured, so the splice always actually lands.
#
# These are SOURCE pins and need no running panel.
#   run: bash tests/git-deploy-pre-build-rce-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AGENT_ROUTES="$REPO/panel/agent/src/routes/git_build.rs"
AGENT_SVC="$REPO/panel/agent/src/services/git_build.rs"
BACKEND="$REPO/panel/backend/src/routes/git_deploys.rs"
FRONTEND="$REPO/panel/frontend/src/pages/GitDeploys.tsx"

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
has()   { grep -q  -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }
hasE()  { grep -qE -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }
hasNot()  { grep -q  -- "$2" "$1" 2>/dev/null && bad "$3" || ok "$3"; }
hasNotE() { grep -qE -- "$2" "$1" 2>/dev/null && bad "$3" || ok "$3"; }
# Comment-blind variants. These two pins describe the retired thing BY NAME in
# the comment that documents why it was retired, so a raw grep would match
# that prose and fail on correct code (the "source-pin PROSE trap").
strip_comments() { sed -E 's|[[:space:]]*//.*$||' "$1"; }
# Counting grep, not -q: a boolean grep piped from a producer quits at its
# first match and can die of SIGPIPE mid-write, which under `set -o
# pipefail` reads a successful match as a failure (s335). Redirect the count
# away instead — the call site's pass/fail meaning is unchanged.
codeNot() { strip_comments "$1" | grep -c -- "$2" >/dev/null && bad "$3" || ok "$3"; }

echo "== A. the host-exec route is gone, not just hardened =="

hasNot  "$AGENT_ROUTES" 'fn pre_build_hook'          "pre_build_hook handler function no longer exists"
hasNot  "$AGENT_ROUTES" 'struct PreBuildHookRequest'  "PreBuildHookRequest struct no longer exists"
hasNot  "$AGENT_ROUTES" '"/git/pre-build-hook"'       "the /git/pre-build-hook route is no longer registered"
hasNotE "$AGENT_ROUTES" 'safe_command\("sh"\)'         "the agent's git_build routes no longer shell out to sh at all"
codeNot "$BACKEND"      '/git/pre-build-hook'          "neither backend deploy path still calls /git/pre-build-hook"

echo
echo "== B. the whitelist moved to auto_detect, unchanged content =="

has  "$AGENT_ROUTES" 'const ALLOWED_PRE_BUILD'                   "ALLOWED_PRE_BUILD whitelist still exists"
N_ENTRIES=$(sed -n '/const ALLOWED_PRE_BUILD/,/\];/p' "$AGENT_ROUTES" 2>/dev/null | grep -c '^\s*"')
if [ "$N_ENTRIES" -eq 9 ]; then
  ok "whitelist still carries exactly its original 9 entries ($N_ENTRIES)"
else
  bad "whitelist entry count drifted: expected 9, found $N_ENTRIES"
fi
hasE "$AGENT_ROUTES" 'if !ALLOWED_PRE_BUILD\.contains\(&cmd\)'   "auto_detect validates pre_build_cmd against the whitelist before use"
has  "$AGENT_ROUTES" 'pre_build_cmd: Option<String>'             "AutoDetectRequest carries the whitelisted override field"
has  "$AGENT_ROUTES" 'fn auto_detect'                            "auto_detect handler still exists (whitelist's new home)"

echo
echo "== C. the whitelisted string only ever becomes Dockerfile text =="

hasE "$AGENT_SVC" 'pre_build_override: Option<&str>'                       "auto_generate_dockerfile takes the override as a parameter"
hasE "$AGENT_SVC" 'Result<\(String, bool, Option<String>\), String>'       "auto_generate_dockerfile returns (path, applied, note)"
hasE "$AGENT_ROUTES" 'git_build::auto_generate_dockerfile\(' "auto_detect still calls auto_generate_dockerfile (route -> service wiring intact)"
has  "$AGENT_SVC" 'fn node_install_run'   "Node install-line resolver exists"
has  "$AGENT_SVC" 'fn pip_install_run'    "Python install-line resolver exists"
has  "$AGENT_SVC" 'corepack enable'       "yarn/pnpm overrides get a corepack prefix (neither is on node:20-alpine's PATH by default)"

echo
echo "== D. mismatch handling: never a broken Dockerfile, never a silent no-op =="

has "$AGENT_SVC" "repo already has a Dockerfile — add the install step as a RUN line there" \
  "a committed Dockerfile refuses to silently absorb an override"
has "$AGENT_SVC" "doesn't apply to this project type — using the default install step" \
  "a cross-language override (e.g. bundle install against Node) falls back to the default, explained"
has "$AGENT_SVC" "has no matching install step for a Go project — ignored" \
  "Go has no ALLOWED_PRE_BUILD entry and says so explicitly"
has "$AGENT_SVC" "has no matching install step for a static site — ignored" \
  "a static site has no install step and says so explicitly"

echo
echo "== E. PHP branch: the literal whitelisted string is what actually runs =="

codeNot "$AGENT_SVC" 'composer\.phar'                      "the local composer.phar invocation is gone (was never runnable as the literal 'composer install' string)"
has    "$AGENT_SVC" '\-\-install-dir=/usr/local/bin \-\-filename=composer' "composer is bootstrapped as a global binary so the whitelisted 'composer install' string is really what runs"

echo
echo "== F. build_image: the timeout now covers the folded-in install step, and actually terminates on expiry =="

hasE   "$AGENT_SVC" 'std::time::Duration::from_secs\(900\)' "build_image's timeout absorbed the old separate 300s pre-build budget (600s -> 900s)"
N_KOD=$(grep -c 'kill_on_drop(true)' "$AGENT_SVC" 2>/dev/null); N_KOD=${N_KOD:-0}
if [ "$N_KOD" -ge 1 ]; then
  ok "build_image's docker-build Command carries kill_on_drop(true) ($N_KOD occurrence(s) in the crate)"
else
  bad "no kill_on_drop(true) found — a timed-out docker build can orphan past its deadline"
fi

echo
echo "== G. backend: both deploy paths gate on agent version and splice pre_build_cmd through =="

has "$BACKEND" 'pub(crate) const PRE_BUILD_SPLICE_MIN_AGENT'  "PRE_BUILD_SPLICE_MIN_AGENT version gate constant exists"
N_GATE=$(grep -c 'PRE_BUILD_SPLICE_MIN_AGENT,' "$BACKEND" 2>/dev/null); N_GATE=${N_GATE:-0}
if [ "$N_GATE" -ge 2 ]; then
  ok "both deploy paths (interactive + scheduled) call require_agent_at_least with the new gate ($N_GATE call sites)"
else
  bad "expected 2 require_agent_at_least call sites using PRE_BUILD_SPLICE_MIN_AGENT, found $N_GATE"
fi
N_SPLICE=$(grep -c '"pre_build_cmd": pre_build_cmd,' "$BACKEND" 2>/dev/null); N_SPLICE=${N_SPLICE:-0}
if [ "$N_SPLICE" -ge 2 ]; then
  ok "both deploy paths pass pre_build_cmd through to /git/auto-detect ($N_SPLICE call sites)"
else
  bad "expected 2 auto-detect calls carrying pre_build_cmd, found $N_SPLICE"
fi
hasE "$BACKEND" 'fn spawn_deploy_task'   "spawn_deploy_task (interactive deploy) still exists"
hasE "$BACKEND" 'fn trigger_deploy_task' "trigger_deploy_task (scheduled deploy) still exists"

echo
echo "== H. the bonus bug: Nixpacks success no longer silently strands a configured pre-build command =="

N_SKIP=$(grep -c 'if pre_build_cmd.is_some() {' "$BACKEND" 2>/dev/null); N_SKIP=${N_SKIP:-0}
if [ "$N_SKIP" -ge 2 ]; then
  ok "both deploy paths branch on pre_build_cmd before deciding whether to try Nixpacks ($N_SKIP sites)"
else
  bad "expected both deploy paths to gate Nixpacks on pre_build_cmd, found $N_SKIP sites"
fi

echo
echo "== I. neighboring surfaces are untouched, not silently widened or dropped =="

has "$AGENT_ROUTES" 'fn run_hook'                              "run_hook (docker-exec into an already-deployed container) is untouched"
has "$AGENT_ROUTES" 'is_safe_hook_command'                     "run_hook's char-blacklist gate is untouched"
has "$REPO/panel/backend/src/routes/mod.rs" 'pub fn is_safe_shell_command' \
  "the backend's own save-time filter (weaker than ALLOWED_PRE_BUILD, shared with post_deploy_cmd) is untouched"

echo
echo "== J. frontend copy no longer promises a host-side effect =="

hasNot "$FRONTEND" 'Runs in the git repo directory before docker build' \
  "the old (now false) 'runs before docker build, on the host' copy is gone"
has    "$FRONTEND" 'Only applies when DockPanel generates the Dockerfile for you' \
  "the field's copy now matches what it actually does"

echo
printf 'passed %d, failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
