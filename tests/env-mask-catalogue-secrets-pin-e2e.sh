#!/usr/bin/env bash
# env-mask-catalogue-secrets-pin-e2e.sh — s447
#
# Pins docker_apps.rs's `GET /apps/{id}/env` mask gap named in
# [[project_dockpanel_tech_debt_p185]] §B: the route's mask condition was a
# substring heuristic (PASSWORD|SECRET|KEY|TOKEN|CREDENTIAL|AUTH) that never
# consulted the catalogue's own `secret: true` flag, so a declared-secret
# name that doesn't happen to contain one of those substrings was returned
# in the clear. 8 such names measured at HEAD before the fix: DB_PASS,
# POSTGRES_PWD, SURREAL_PASS, GOTIFY_DEFAULTUSER_PASS, PLEX_CLAIM,
# DATABASE_URL, CLICKHOUSE_DATABASE_URL, REDASH_DATABASE_URL.
#
# Fix: `catalogue_secret_env(name)` (services/docker_apps.rs, mirrors the
# existing `catalogue_non_secret_env`) OR'd into the route's mask condition.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  docker_apps.rs env mask — catalogue secret coverage (s447)"
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
[ -f "$SERVICES" ] || { bad "SETUP subject missing: $SERVICES"; exit 1; }
[ -f "$ROUTES" ] || { bad "SETUP subject missing: $ROUTES"; exit 1; }
SERVICES_SRC=$(code "$SERVICES")
ROUTES_SRC=$(code "$ROUTES")

# ── §A catalogue_secret_env exists and reads the catalogue's own flag ───────

CSE=$(flat "$(fnbody "$SERVICES_SRC" "catalogue_secret_env")")
if [ -z "$CSE" ]; then
  bad "A0 could not extract catalogue_secret_env from services/docker_apps.rs"
else
  ok "A0 catalogue_secret_env extracted"
  if has "$CSE" '\.secret'; then
    ok "A1 catalogue_secret_env reads the catalogue's own secret flag"
  else
    bad "A1 catalogue_secret_env doesn't read v.secret — not actually checking the catalogue"
  fi
fi

# ── §B get_env ORs the catalogue check into the mask condition ──────────────

GETENV=$(flat "$(fnbody "$ROUTES_SRC" "get_env")")
if [ -z "$GETENV" ]; then
  bad "B0 could not extract get_env from routes/docker_apps.rs"
else
  ok "B0 get_env extracted"
  if has "$GETENV" 'catalogue_secret_env'; then
    ok "B1 get_env calls catalogue_secret_env"
  else
    bad "B1 get_env never calls catalogue_secret_env — the route and the fix are disconnected"
  fi
  if has "$GETENV" '\|\| docker_apps::catalogue_secret_env\(&k\)'; then
    ok "B2 catalogue_secret_env is OR'd in (widens masking, never narrows the substring heuristic)"
  else
    bad "B2 catalogue_secret_env is not OR'd into is_sensitive — verify by hand it can't narrow the mask instead of widening it"
  fi
fi

# ── §C the 8 measured names are declared secret:true in the catalogue ───────

echo "── §C the 8 catalogue names the substring heuristic missed ──"
for NAME in DB_PASS POSTGRES_PWD SURREAL_PASS GOTIFY_DEFAULTUSER_PASS PLEX_CLAIM DATABASE_URL CLICKHOUSE_DATABASE_URL REDASH_DATABASE_URL; do
  if grep -qE "name: \"$NAME\".*secret: true" "$SERVICES"; then
    ok "C:$NAME declared secret: true in the catalogue"
  else
    bad "C:$NAME has no secret:true EnvVarDef — control failed, this name was never actually a control case"
  fi
done

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
