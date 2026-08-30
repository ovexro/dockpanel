#!/usr/bin/env bash
# Regression pins for the s429 dockpanel-fanout follow-up: a cron/shell-command
# chaining bypass (root RCE) and an unauthenticated Dozzle-sidecar access gap.
#
# 1. CRON/SHELL-COMMAND CHAINING BYPASS (CRITICAL). is_safe_cron_command
#    (panel/agent/src/services/command_filter.rs) and the backend's
#    is_safe_shell_command (panel/backend/src/routes/mod.rs, shared by cron,
#    pre_build_cmd, post_deploy_cmd) only rejected "; " (semicolon+space) and
#    verb-anchored "|sh"/"|bash"/"| sh"/"| bash" — a bare `;` or bare `|` with
#    no trailing space/verb ("id;whoami", "id|whoami") passed both filters
#    cleanly. The command reaches `crontab -u root -` (root's real crontab)
#    and POST /crons/run executes it via `bash -c` immediately, as the agent
#    (unsandboxed root). Reachable by an ordinary NON-ADMIN site owner via
#    SITE_CALLER_PREDICATE, not just an admin. Verified live with real PoC
#    strings by both the finder and an independent skeptic before the fix
#    landed. Fix: reject any bare `;`, and reject any run of `|` whose length
#    isn't exactly 2 — `&&`/`||` remain explicitly allowed (the whole reason
#    this isn't a blanket metacharacter ban). Mutation-tested: the paired Rust
#    unit tests (command_filter.rs, routes/mod.rs) were confirmed red against
#    the pre-fix logic before being confirmed green against the fix.
#
# 2. DOZZLE SIDECAR: UNAUTHENTICATED ACCESS TO EVERY CONTAINER ON THE SHARED
#    HOST (CRITICAL). The v2.179.0 docker-socket-proxy sidecar correctly
#    restricts WHAT Dozzle can ask the Docker daemon (deny-by-default ACL,
#    CONTAINERS=1/EVENTS=1/INFO=1 only) but nothing gated WHO could reach
#    Dozzle itself. Deploying the template with a domain — the panel's own
#    advertised one-click, DNS-automated primary use case — produced a fully
#    public page that lists and inspects every container on the shared Docker
#    host, including plaintext env vars (DB passwords, JWT secrets, ...) for
#    every OTHER project sharing the box. Proven with real evidence from this
#    box's own running containers during the audit. Dozzle's own real auth
#    (verified against its own docs, not guessed) needs either a mounted
#    users.yml with a bcrypt hash inside, or forward-proxy headers — no
#    pure-env-var auth exists, so the fix cannot be "add a required env var
#    to the template." Fixed by forcing nginx auth_basic on the domain at
#    deploy time for any TEMPLATE_SIDECAR-backed app, reusing the same
#    htpasswd mechanism Sites' own password-protect feature already uses.
#
# These are SOURCE pins and need no running panel.
#   run: bash tests/chain-operator-bypass-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CMDFILTER="$REPO/panel/agent/src/services/command_filter.rs"
BACKEND_MOD="$REPO/panel/backend/src/routes/mod.rs"
DOCKER_APPS_BACKEND="$REPO/panel/backend/src/routes/docker_apps.rs"

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
has()   { grep -q  -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }
hasE()  { grep -qE -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }
hasNot()  { grep -q  -- "$2" "$1" 2>/dev/null && bad "$3" || ok "$3"; }

echo "== A. cron chaining bypass: bare ; and bare | are rejected, && / || kept =="

hasE "$CMDFILTER" "if cmd.contains\\('\`'\\) \\|\\| cmd.contains\\(\"\\\$\\(\"\\) \\|\\| cmd.contains\\(';'\\)" \
  "is_safe_cron_command rejects a bare ; as a raw character, unconditionally (also proves the old space-anchored \"| \" check was replaced, not merely supplemented)"
has "$CMDFILTER" 'if i - start != 2' \
  "is_safe_cron_command scans runs of | and rejects anything that isn't exactly a pair (bare | rejected, || kept)"
has "$CMDFILTER" 'fn test_cron_bare_chain_operators_rejected_paired_operators_kept' \
  "a dedicated regression test exists for the bypass PoC strings + the && / || legitimate case"

echo
echo "== B. backend is_safe_shell_command carries the identical fix (cron/pre_build_cmd/post_deploy_cmd save-time gate) =="

hasE "$BACKEND_MOD" 'if cmd.contains\(.;.\) \{' \
  "is_safe_shell_command rejects a bare ; unconditionally"
has "$BACKEND_MOD" 'if i - start != 2' \
  "is_safe_shell_command scans runs of | the same way as the agent-side fix"
has "$BACKEND_MOD" 'fn shell_command_rejects_bare_chain_operators_keeps_paired_operators' \
  "a dedicated regression test exists on the backend side too"

echo
echo "== C. Dozzle sidecar: forced nginx password-protection on deploy =="

has "$DOCKER_APPS_BACKEND" 'const SIDECAR_TEMPLATES: &\[&str\] = &\["dozzle"\]' \
  "the deploy handler names dozzle as a sidecar-backed template needing forced protection"
has "$DOCKER_APPS_BACKEND" '"/nginx/password-protect"' \
  "deploying a sidecar template calls the agent's nginx password-protect endpoint directly"
hasE "$DOCKER_APPS_BACKEND" 'SIDECAR_TEMPLATES\.contains\(&template\.as_str\(\)\)' \
  "the protection call is gated on the deployed template actually being in SIDECAR_TEMPLATES"
N_DOMAIN_GUARDS=$(grep -c 'if let Some(ref domain) = deploy_domain' "$DOCKER_APPS_BACKEND" 2>/dev/null); N_DOMAIN_GUARDS=${N_DOMAIN_GUARDS:-0}
if [ "$N_DOMAIN_GUARDS" -ge 4 ]; then
  ok "protection is only attempted when a domain is actually being bound ($N_DOMAIN_GUARDS deploy_domain guards total, incl. the new one)"
else
  bad "expected at least 4 'if let Some(ref domain) = deploy_domain' guards (incl. the new auth one), found $N_DOMAIN_GUARDS"
fi
has "$DOCKER_APPS_BACKEND" 'system_log::log_event' \
  "the generated credentials are also logged to System Logs as a durable backup, not shown once with no recovery path"
hasE "$DOCKER_APPS_BACKEND" 'emit\("auth", "Password-protecting the app", "error"' \
  "a failed protection call surfaces as an explicit error step, not a silent no-op"

echo
printf 'passed %d, failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
