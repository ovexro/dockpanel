#!/usr/bin/env bash
# git-clone-ssrf-pin-e2e.sh — s442
#
# ONE PROPERTY: before THIS process (panel/agent) invokes a `git clone`/
# subprocess against a caller-supplied `repo_url`, it resolves that host
# itself and refuses an internal/private address.
#
# `panel/backend/src/helpers.rs::validate_repo_url_not_internal` already
# checks `git_deploys.repo_url` — but it runs on the PANEL host, at WRITE
# time. The actual clone is dispatched later (immediately, on a scheduled
# pull, or on a redeploy — an unbounded interval) to THIS process, on a
# DIFFERENT host, via a plain `git clone`/`git fetch` subprocess with no
# validation of its own. A DNS answer that differs between "how the panel
# resolved this hostname" and "how this fleet member resolves it right now"
# sails straight through untouched — the two checks don't even share a
# network vantage point, let alone a moment in time. This is the exact class
# `helpers.rs`'s `resolve_validated`/`pinned_client` were built to close for
# `reqwest` call sites; a subprocess `git clone` has no equivalent hook, so
# the guard here is the same resolve-then-check pattern the codebase already
# accepts for `check_tcp`/`check_ping` (see ssrf_guard.rs's own module docs
# for the full reasoning and its accepted residual TOCTOU window).
#
# Reachable via THREE independent code paths, all agent-side and none
# previously guarded: `POST /git/clone` -> git_build.rs::clone_or_pull,
# `POST /deploy/run` -> deploy.rs::clone_or_pull, and `POST /deploy/atomic`
# -> deploy.rs::atomic_deploy (its own inline clone, not a clone_or_pull
# call at all).
#
# §A the ssrf_guard module exists, is registered, and carries every
#    internal-address range this class of guard needs (including the cloud
#    metadata block, the most commonly abused SSRF target).
# §B the agent crate depends on `url` (needed to parse https/http/ssh
#    repo_urls; the scp-like git@host:path shorthand is hand-parsed).
# §C all three reachable clone paths call the guard.
# §D position: the guard runs BEFORE the git subprocess is spawned, in
#    every one of the three functions.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  agent git-clone SSRF/DNS-rebind guard — source pins (s442)"
echo "=================================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

code() {
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has()  { grep -qE -- "$2" <<< "$1"; }

fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn "(") && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

offset() {
  local body="$1" needle="$2"
  local head="${body%%"$needle"*}"
  if [ "$head" = "$body" ]; then
    echo -1
  else
    echo "${#head}"
  fi
}

before() {
  local body="$1" first="$2" second="$3"
  local o1 o2
  o1=$(offset "$body" "$first")
  o2=$(offset "$body" "$second")
  [ "$o1" != -1 ] && [ "$o2" != -1 ] && [ "$o1" -lt "$o2" ]
}

GUARD=panel/agent/src/services/ssrf_guard.rs
GUARD_C=$(code "$GUARD")
MOD=panel/agent/src/services/mod.rs
GITBUILD=panel/agent/src/services/git_build.rs
GITBUILD_C=$(code "$GITBUILD")
DEPLOY=panel/agent/src/services/deploy.rs
DEPLOY_C=$(code "$DEPLOY")

GB_CLONE_BODY=$(fnbody "$GITBUILD_C" "clone_or_pull")
DEP_CLONE_BODY=$(fnbody "$DEPLOY_C" "clone_or_pull")
DEP_ATOMIC_BODY=$(fnbody "$DEPLOY_C" "atomic_deploy")

# ── §A the guard module exists and covers the internal-address ranges ────
echo "── §A ssrf_guard exists and knows the ranges that matter most ──"

if [ -f "$GUARD" ]; then
  ok "A1 $GUARD exists"
else
  bad "A1 $GUARD is missing — every arm below measures nothing"
fi

if has "$GUARD_C" 'pub async fn validate_repo_url_not_internal'; then
  ok "A2 validate_repo_url_not_internal is a public entry point"
else
  bad "A2 no public validate_repo_url_not_internal in ssrf_guard.rs"
fi

if has "$GUARD_C" 'is_loopback\(\)' && has "$GUARD_C" 'is_private\(\)' && has "$GUARD_C" 'is_link_local\(\)'; then
  ok "A3 loopback/private/link-local are all covered"
else
  bad "A3 one of loopback/private/link-local is missing from the range checks"
fi

if has "$GUARD_C" 'o\[0\] == 169'; then
  ok "A4 the cloud-metadata block (169.254.x, the most commonly abused SSRF target) is covered"
else
  bad "A4 no explicit 169.x check — cloud metadata endpoints would not be blocked"
fi

if has "$GUARD_C" 'o\[0\] == 100 && \(o\[1\] & 0xC0\) == 0x40'; then
  ok "A5 CGNAT (100.64.0.0/10) is covered"
else
  bad "A5 CGNAT range check is missing"
fi

if has "$GUARD_C" 'fn v6_is_internal'; then
  ok "A6 IPv6 has its own guard (not just IPv4)"
else
  bad "A6 no IPv6 handling — a v6 literal or AAAA record would sail through"
fi

if has "$GUARD_C" 'tokio::net::lookup_host'; then
  ok "A7 the guard actually resolves the hostname (not just literal-IP checks)"
else
  bad "A7 no DNS resolution — a hostname-based repo_url would bypass every check above"
fi

# ── §B the agent crate can parse repo_url ─────────────────────────────────
echo "── §B panel/agent depends on url (for https/http/ssh parsing) ──"

if grep -qE '^url\s*=' panel/agent/Cargo.toml; then
  ok "B1 panel/agent/Cargo.toml depends on url"
else
  bad "B1 no url dependency in panel/agent/Cargo.toml — the guard would not compile"
fi

if has "$GUARD_C" "strip_prefix\\(\"git@\"\\)"; then
  ok "B2 the scp-like git@host:path shorthand is hand-parsed (the url crate has no scheme for it)"
else
  bad "B2 no scp-like shorthand handling — git@host:path repo_urls would be misparsed or rejected"
fi

# ── §C the module is registered and reachable ─────────────────────────────
echo "── §C ssrf_guard is registered as a module ──"

if grep -qE '^pub mod ssrf_guard;' "$MOD"; then
  ok "C1 pub mod ssrf_guard; is registered in services/mod.rs"
else
  bad "C1 ssrf_guard is not registered — it exists on disk but is unreachable from the crate"
fi

# ── §D all three reachable clone paths call the guard, in position ────────
echo "── §D all 3 reachable clone paths call the guard BEFORE spawning git ──"

if has "$GB_CLONE_BODY" 'ssrf_guard::validate_repo_url_not_internal\(repo_url\)\.await\?'; then
  ok "D1 git_build.rs::clone_or_pull calls the guard"
else
  bad "D1 git_build.rs::clone_or_pull does not call the guard"
fi

if before "$GB_CLONE_BODY" "validate_repo_url_not_internal" "cmd.output()"; then
  ok "D2 git_build.rs::clone_or_pull validates before spawning the git subprocess"
else
  bad "D2 git_build.rs::clone_or_pull's guard runs after (or never before) the subprocess spawn"
fi

if has "$DEP_CLONE_BODY" 'ssrf_guard::validate_repo_url_not_internal\(repo_url\)\.await\?'; then
  ok "D3 deploy.rs::clone_or_pull calls the guard"
else
  bad "D3 deploy.rs::clone_or_pull does not call the guard"
fi

if before "$DEP_CLONE_BODY" "validate_repo_url_not_internal" "cmd.output()"; then
  ok "D4 deploy.rs::clone_or_pull validates before spawning the git subprocess"
else
  bad "D4 deploy.rs::clone_or_pull's guard runs after (or never before) the subprocess spawn"
fi

if has "$DEP_ATOMIC_BODY" 'ssrf_guard::validate_repo_url_not_internal\(repo_url\)\.await\?'; then
  ok "D5 deploy.rs::atomic_deploy calls the guard (its own inline clone, not a clone_or_pull call)"
else
  bad "D5 deploy.rs::atomic_deploy does not call the guard — its inline clone would stay unguarded"
fi

if before "$DEP_ATOMIC_BODY" "validate_repo_url_not_internal" "cmd.output()"; then
  ok "D6 deploy.rs::atomic_deploy validates before spawning the git subprocess"
else
  bad "D6 deploy.rs::atomic_deploy's guard runs after (or never before) the subprocess spawn"
fi

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
