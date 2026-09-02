#!/usr/bin/env bash
# registry-pull-credentials-pin-e2e.sh — s447
#
# Pins the docker_apps.rs finding named in [[project_dockpanel_tech_debt_p185]]
# §C: `GET /apps/registries` read a literal `/root/.docker/config.json`, but
# every `docker` CLI invocation runs through `safe_command`, which points
# `DOCKER_CONFIG` at `crate::safe_cmd::DOCKER_CONFIG_DIR`
# (`/var/lib/dockpanel/docker`) — so a login always landed somewhere the
# route never looked, and the panel's "Configured registries" list was
# always empty. Separately, every bollard `create_image` pull (template
# deploy, app update, change-image, and compose deploy) passed
# `credentials: None` regardless, so a private-image deploy failed even
# immediately after a successful login.
#
# Fix: `list_registries` reads `DOCKER_CONFIG_DIR`; a new
# `registry_credentials_for(image)` helper (services/docker_apps.rs) looks up
# saved credentials by the image's parsed registry and is wired into all 4
# production `create_image` call sites (3 in docker_apps.rs, 1 in
# compose.rs). A 5th `create_image` call exists only in a `#[cfg(test)]`
# probe helper and is deliberately excluded — it pulls a public test image
# and needs no credentials.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  docker_apps.rs registry pull credentials (s447)"
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
flat() { tr '\n' ' ' <<< "$1" | tr -s ' '; }

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

SERVICES=panel/agent/src/services/docker_apps.rs
ROUTES=panel/agent/src/routes/docker_apps.rs
COMPOSE=panel/agent/src/services/compose.rs
for f in "$SERVICES" "$ROUTES" "$COMPOSE"; do
  [ -f "$f" ] || { bad "SETUP subject missing: $f"; exit 1; }
done
SERVICES_SRC=$(code "$SERVICES")
ROUTES_SRC=$(code "$ROUTES")

# ── §A list_registries reads the config DOCKER_CONFIG actually points at ────

LISTREG=$(flat "$(fnbody "$ROUTES_SRC" "list_registries")")
if [ -z "$LISTREG" ]; then
  bad "A0 could not extract list_registries"
elif has "$LISTREG" 'DOCKER_CONFIG_DIR'; then
  ok "A1 list_registries reads crate::safe_cmd::DOCKER_CONFIG_DIR, not a hardcoded /root path"
else
  bad "A1 list_registries still reads a path other than DOCKER_CONFIG_DIR — logins land somewhere this route never looks"
fi

# ── §B registry_credentials_for exists and is reachable outside this module ─

RCF=$(flat "$(fnbody "$SERVICES_SRC" "registry_credentials_for")")
if [ -z "$RCF" ]; then
  bad "B0 could not extract registry_credentials_for"
else
  ok "B0 registry_credentials_for extracted"
  if has "$RCF" 'DOCKER_CONFIG_DIR' && has "$RCF" 'auths'; then
    ok "B1 registry_credentials_for reads DOCKER_CONFIG_DIR's config.json auths map"
  else
    bad "B1 registry_credentials_for doesn't read the same auths map docker login writes"
  fi
fi
if grep -qE 'pub\(crate\) fn registry_credentials_for' "$SERVICES"; then
  ok "B2 registry_credentials_for is pub(crate) — reachable from compose.rs"
else
  bad "B2 registry_credentials_for is not pub(crate) — compose.rs cannot call it"
fi

# ── §C every production create_image site passes real credentials ──────────
# Scoped to code before the first #[cfg(test)] module — a test-only pull of a
# public image needs no credentials and must not inflate this count.

PROD_SERVICES=$(awk '/^#\[cfg\(test\)\]/{exit} {print}' "$SERVICES")
CI_COUNT=$(grep -c 'create_image(' <<< "$PROD_SERVICES")
RCF_COUNT=$(grep -c 'registry_credentials_for(' <<< "$PROD_SERVICES")
# 3 create_image call sites in docker_apps.rs; registry_credentials_for
# appears once as its own definition plus once per call site = 4.
if [ "$CI_COUNT" -eq 3 ]; then
  ok "C1 docker_apps.rs has 3 production create_image sites (test-only probe excluded)"
else
  bad "C1 docker_apps.rs has $CI_COUNT production create_image sites, expected 3 — this suite's count needs updating"
fi
if [ "$RCF_COUNT" -eq 4 ]; then
  ok "C2 registry_credentials_for appears 4 times (1 definition + 3 call sites) — every production pull is wired"
else
  bad "C2 registry_credentials_for appears $RCF_COUNT times, expected 4 — a create_image site still passes None"
fi

COMPOSE_SRC=$(code "$COMPOSE")
DEPLOY_SVC=$(flat "$(fnbody "$COMPOSE_SRC" "deploy_service")")
if has "$DEPLOY_SVC" 'registry_credentials_for'; then
  ok "C3 compose.rs deploy_service passes registry_credentials_for, not None"
else
  bad "C3 compose.rs deploy_service still pulls with credentials: None"
fi

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
