#!/usr/bin/env bash
# registry-ollama-arg-validation-pin-e2e.sh — s439
#
# Pins the docker_apps.rs registry_login/ollama_pull/ollama_delete finding
# named in [[project_dockpanel_tech_debt_p4]] (long-standing INFO carry: "exec
# denylist substring theater... registry_login/ollama_pull accept an
# arbitrary host") and confirmed still open at HEAD (v2.194.0, ab9a205)
# before this fix. Same DiD shape as the s438 nginx.rs/remote_backup.rs
# closure ([[project_dockpanel_tech_debt_p176]]): every one of these 4
# handlers passes an admin-supplied string straight into a `docker`/`ollama`
# CLI argument via `safe_command(...).args([...])` (no shell, so classic `;`/
# `|` injection is not reachable — args() passes each token verbatim), but
# `safe_command` does nothing to stop the string ITSELF being read as a CLI
# FLAG when it lands in argv right where a flag is expected.
#
#   §A registry_login had ZERO format validation on `body.server` (only an
#      emptiness check) before `docker login <server> -u <user>
#      --password-stdin` — a server starting with `-` is parsed by docker's
#      own arg parser as an option in that position, not the registry host.
#   §B registry_logout had the identical gap on `server` before `docker
#      logout <server>`.
#   §C/§D ollama_pull_model/ollama_delete_model already validated length and
#      character class, but neither rejected a LEADING `-` before `docker
#      exec <container_id> ollama pull/rm <model>` — the same argv-position
#      hazard, just missing the one guard `is_valid_image_ref` (used a few
#      hundred lines away for `change_image`'s `image` field, the established
#      sibling for this exact threat class in this same file) already has.
#
# The fix for §A/§B reuses `is_valid_image_ref` directly (same file, same
# crate, matching the fix-precedent of reusing an existing sibling validator
# instead of inventing a near-duplicate one) rather than a fresh domain
# validator, because a registry server is commonly `host:port` or a bare
# single-label host (`localhost:5000`) — shapes `is_valid_domain` (which
# requires a dot and rejects `:`) would incorrectly reject.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  docker_apps.rs registry/ollama arg validation — source pins (s439)"
echo "=============================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# Strip comments before matching (per [[feedback_source_pin_prose_trap]] —
# this file's own header spells the tokens the arms grep for).
code() {
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*///.*$}{}gm;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

has()  { grep -qE -- "$2" <<< "$1"; }
flat() { tr '\n' ' ' <<< "$1" | tr -s ' '; }

# A function body, bounded on ITS OWN braces (s323 fix — a window ending at
# the successor's match position swallows the next function's declaration).
fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn) && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

APPS=panel/agent/src/routes/docker_apps.rs
[ -f "$APPS" ] || { bad "SETUP subject missing: $APPS"; exit 1; }
APPS_SRC=$(code "$APPS")

# ── §A  registry_login validates body.server before `docker login` ──────────

LOGIN=$(flat "$(fnbody "$APPS_SRC" "registry_login")")
if [ -z "$LOGIN" ]; then
  bad "A0 could not extract docker_apps::registry_login"
else
  ok "A0 docker_apps::registry_login extracted"
  if has "$LOGIN" 'is_valid_image_ref\(&body\.server\)'; then
    ok "A1 registry_login validates body.server with is_valid_image_ref"
  else
    bad "A1 registry_login has no format validation on body.server — a leading '-' is parsed as a docker login flag, not a registry host"
  fi
  CHK_AT=$(grep -bo 'is_valid_image_ref(&body.server)' <<< "$LOGIN" | head -1 | cut -d: -f1)
  SPAWN_AT=$(grep -bo '"login"' <<< "$LOGIN" | head -1 | cut -d: -f1)
  if [ -n "$CHK_AT" ] && [ -n "$SPAWN_AT" ] && [ "$CHK_AT" -lt "$SPAWN_AT" ]; then
    ok "A2 the validation precedes the docker login spawn"
  else
    bad "A2 the validation does not precede the docker login spawn (check@${CHK_AT:-none} spawn@${SPAWN_AT:-none})"
  fi
fi

# ── §B  registry_logout validates server before `docker logout` ─────────────

LOGOUT=$(flat "$(fnbody "$APPS_SRC" "registry_logout")")
if [ -z "$LOGOUT" ]; then
  bad "B0 could not extract docker_apps::registry_logout"
else
  ok "B0 docker_apps::registry_logout extracted"
  if has "$LOGOUT" 'is_valid_image_ref\(server\)'; then
    ok "B1 registry_logout validates server with is_valid_image_ref"
  else
    bad "B1 registry_logout has no format validation on server — a leading '-' is parsed as a docker logout flag, not a registry host"
  fi
  CHK_AT=$(grep -bo 'is_valid_image_ref(server)' <<< "$LOGOUT" | head -1 | cut -d: -f1)
  SPAWN_AT=$(grep -bo '"logout"' <<< "$LOGOUT" | head -1 | cut -d: -f1)
  if [ -n "$CHK_AT" ] && [ -n "$SPAWN_AT" ] && [ "$CHK_AT" -lt "$SPAWN_AT" ]; then
    ok "B2 the validation precedes the docker logout spawn"
  else
    bad "B2 the validation does not precede the docker logout spawn (check@${CHK_AT:-none} spawn@${SPAWN_AT:-none})"
  fi
fi

# ── §C  ollama_pull_model rejects a leading '-' in the model name ───────────

PULL=$(flat "$(fnbody "$APPS_SRC" "ollama_pull_model")")
if [ -z "$PULL" ]; then
  bad "C0 could not extract docker_apps::ollama_pull_model"
else
  ok "C0 docker_apps::ollama_pull_model extracted"
  if has "$PULL" 'model\.starts_with\(.-.\)'; then
    ok "C1 ollama_pull_model rejects a model name starting with '-'"
  else
    bad "C1 ollama_pull_model has no leading-hyphen guard — a model name starting with '-' is parsed as an ollama pull flag"
  fi
fi

# ── §D  ollama_delete_model rejects a leading '-' in the model name ─────────

DELETE=$(flat "$(fnbody "$APPS_SRC" "ollama_delete_model")")
if [ -z "$DELETE" ]; then
  bad "D0 could not extract docker_apps::ollama_delete_model"
else
  ok "D0 docker_apps::ollama_delete_model extracted"
  if has "$DELETE" 'model\.starts_with\(.-.\)'; then
    ok "D1 ollama_delete_model rejects a model name starting with '-'"
  else
    bad "D1 ollama_delete_model has no leading-hyphen guard — a model name starting with '-' is parsed as an ollama rm flag"
  fi
fi

echo
echo "----------------------------------------------"
printf '  PASS %d  FAIL %d\n' "$PASS" "$FAIL"
echo "----------------------------------------------"
echo

[ "$FAIL" -eq 0 ] || exit 1
exit 0
