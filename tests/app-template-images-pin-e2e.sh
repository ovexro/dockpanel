#!/usr/bin/env bash
#
# Regression pin for the app-template catalogue's IMAGE REFERENCES and start
# commands — issue #103.
#
# WHAT HAPPENED. Eighteen of the 153 catalogue entries could not be deployed at
# all, because the image reference they pinned did not resolve:
#
#   fifteen pinned a tag the publisher does not publish. The shape was always the
#   same — a bare major-version shorthand (`:1`, `:2`, `:11`, `:26`, `v3`, `v4`)
#   assumed to be a convention these publishers simply do not use. `grafana`
#   publishes `13.1`, never `11`; `keycloak` publishes `26.7`, never `26`;
#   `rocket.chat` has 7814 tags and not one bare major among them.
#
#   two pinned a repository that no longer exists at all (`strapi/strapi` was
#   deleted; the ghcr package `abetlen/stable-diffusion-webui` is gone).
#
#   one pinned a product line that was discontinued in 2020 (`zabbix-appliance`).
#
# None of this was visible from the source, and none of it could be caught by a
# build: a template is a static string, and the failure happens at `docker pull`
# on a stranger's machine. s339 found five of them and recorded eight more as
# "UNKNOWN — Docker Hub rate-limited the sweep"; re-run authenticated at s341,
# ALL EIGHT were dead, plus two nobody had flagged.
#
# WHY THIS PIN IS SHAPED THE WAY IT IS. The tempting arm — "no template may pin a
# bare major tag" — is wrong, and would be the #386 mistake of a screen that
# flags most of its population: `mongo:7`, `mysql:8`, `postgres:16-alpine`,
# `gitea:1` and `prom/prometheus:v3` are all bare-major pins that resolve
# perfectly well, because those publishers DO use that convention. Whether a tag
# exists is a fact about a registry on a given day, not a fact about this commit,
# so it cannot be decided here. Two consequences:
#
#   * this suite pins the SPECIFIC references that were measured dead, so none of
#     them can return by a revert or a copy-paste, and it pins the replacements
#     POSITIVELY so a silent downgrade is caught in the loud direction too
#     (lesson #384: an absence arm alone lets a third thing pass by being absent);
#
#   * the question "does every catalogue tag still resolve TODAY" belongs to
#     `tests/live-surfaces-check.sh`, which asserts things about the world rather
#     than about this commit, and runs daily from a GitHub runner.
#
# WRAP TOLERANCE. Arms that read the TEMPLATE_CMD table canonicalise it by
# deleting whitespace before matching, and never spell a closing delimiter, so a
# rustfmt reflow (which adds line breaks AND a trailing comma) cannot blind them.
# That is lesson #384, which was learned by shipping the defect into the very
# suite written to pin it.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

CATALOGUE=panel/agent/src/services/docker_apps.rs
ICONS=panel/frontend/src/pages/Apps.tsx
[ -f "$CATALOGUE" ] || { echo "missing $CATALOGUE"; exit 1; }

# Canonical form: whitespace deleted. Wrapping and indentation become invisible,
# so one pattern matches the single-line and the rustfmt-wrapped spelling alike.
FLAT=$(tr -d ' \t\n' < "$CATALOGUE")

echo "§1 the catalogue still parses (a broken parser must not read as a clean sweep)"

N=$(awk '
  /static TEMPLATES/ { inside = 1; next }
  inside && /^\];/   { exit }
  inside && /^        id: "/ { n++ }
  END { print n + 0 }
' "$CATALOGUE")
if [ "$N" -ge 100 ]; then
  ok "catalogue parsed: $N templates"
else
  bad "catalogue parse returned $N — the parser has drifted from the source shape (#100)"
  echo; printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"; exit 1
fi

echo
echo "§2 no reference measured UNPULLABLE at s341 is back"

# Every entry here was probed against its registry and classified by the error
# MESSAGE, never by an exit code: 404 "manifest unknown" = the tag is absent;
# 401 on Docker Hub = the repository does not exist anonymously (proven against a
# fabricated control repo that returns the identical 401); 429 = inconclusive and
# was re-run, never recorded as a negative.
DEAD=(
  'axllent/mailpit:v1'
  'bitnami/discourse:3'
  'calcom/cal.com:v4'
  'chatwoot/chatwoot:v3'
  'codercom/code-server:4'
  'dxflrs/garage:v1.0'
  'grafana/grafana:11'
  'mattermost/mattermost-team-edition:10'
  'metabase/metabase:v0.52'
  'outlinewiki/outline:0.82'
  'portainer/portainer-ce:2'
  'quay.io/keycloak/keycloak:26'
  'registry.rocket.chat/rocketchat/rocket.chat:7'
  'strapi/strapi:4'
  'vaultwarden/server:1'
  'woodpeckerci/woodpecker-server:2'
  'zabbix/zabbix-appliance:7.0-latest'
  'ghcr.io/abetlen/stable-diffusion-webui:latest'
)
back=0
for ref in "${DEAD[@]}"; do
  if grep -qF "image:\"$ref\"" <<<"$FLAT"; then
    bad "$ref is pinned again — it was measured unpullable; this app cannot deploy at all"
    back=1
  fi
done
[ "$back" -eq 0 ] && ok "none of the ${#DEAD[@]} unpullable references is present"

echo
echo "§3 each replacement is present (the positive arm — a downgrade must fail loudly too)"

# Each proven with an HTTP 200 manifest request at s341, against a control.
declare -A LIVE=(
  [mailpit]='axllent/mailpit:v1.30'
  [cal-com]='calcom/cal.com:v6.2.0'
  [chatwoot]='chatwoot/chatwoot:latest-ce'
  [code-server]='codercom/code-server:4.131.0'
  [garage]='dxflrs/garage:v2.3.0'
  [grafana]='grafana/grafana:13.1'
  [mattermost]='mattermost/mattermost-team-edition:release-11'
  [metabase]='metabase/metabase:v0.58-lts'
  [outline]='outlinewiki/outline:1.9'
  [portainer]='portainer/portainer-ce:lts'
  [vaultwarden]='vaultwarden/server:1.37.1'
  [keycloak]='quay.io/keycloak/keycloak:26.7'
  [woodpecker]='woodpeckerci/woodpecker-server:v3'
  [rocketchat]='rocketchat/rocket.chat:8.7.0'
)
missing=0
for t in "${!LIVE[@]}"; do
  if grep -qF "image:\"${LIVE[$t]}\"" <<<"$FLAT"; then :; else
    bad "$t no longer pins ${LIVE[$t]} — the resolvable reference was replaced"
    missing=1
  fi
done
[ "$missing" -eq 0 ] && ok "all ${#LIVE[@]} replacement references are present"

echo
echo "§4 the four templates with no maintained official image stay withdrawn"

# Withdrawn rather than repointed: the only pullable candidates were a frozen
# vendor archive (bitnamilegacy, unpatched), a personal-account rebuild, or an
# amd64-only third-party image — and this panel publishes an arm64 build.
for id in discourse zabbix strapi stable-diffusion-webui; do
  if grep -qF "id:\"$id\"" <<<"$FLAT"; then
    bad "template '$id' is back in the catalogue — no maintained official image exists for it"
  else
    ok "'$id' withdrawn"
  fi
done

# A withdrawn template whose icon survives is a severed pair: dead code keyed by
# an id nothing can render.
orphan=0
for id in strapi discourse; do
  if [ -f "$ICONS" ] && grep -qE "^  $id: \(" "$ICONS"; then
    bad "$ICONS still carries an icon for the withdrawn '$id'"
    orphan=1
  fi
done
[ "$orphan" -eq 0 ] && ok "no icon outlives its withdrawn template"

echo
echo "§5 every start command established by running the image is still declared"

# The #101 class: the image's own default does not start a service, so the
# container prints something and exits on every deploy. Each command below was
# established by RUNNING the image and observing BOTH directions — without it the
# container exits, with it the container stays up and answers on its port.
#
# Matched against the whitespace-stripped source and deliberately NOT closed with
# `)`, so a rustfmt reflow — which both breaks lines and adds a trailing comma —
# cannot make these arms silently blind.
declare -A CMD=(
  [minio]='("minio",&["server","/data","--console-address",":9001"'
  [cloudflared]='("cloudflared",&["tunnel","run"'
  [ntfy]='("ntfy",&["serve"'
  [trivy]='("trivy",&["server","--listen","0.0.0.0:8080"'
  [keycloak]='("keycloak",&["start-dev"'
)
lost=0
for id in "${!CMD[@]}"; do
  if grep -qF "${CMD[$id]}" <<<"$FLAT"; then :; else
    bad "TEMPLATE_CMD no longer declares '$id' — that container reverts to printing its default and exiting"
    lost=1
  fi
done
[ "$lost" -eq 0 ] && ok "all ${#CMD[@]} start commands declared"

# Fail-closed on the table itself: an id here that is not in the catalogue is a
# command that can never fire, which reads exactly like coverage.
CMD_IDS=$(awk '/static TEMPLATE_CMD/ {inside=1} inside && /^\];/ {exit}
               inside && match($0, /\("[a-z0-9-]+"/) { print substr($0, RSTART+2, RLENGTH-3) }' "$CATALOGUE")
stray=0
for id in $CMD_IDS; do
  grep -qF "id:\"$id\"" <<<"$FLAT" || { bad "TEMPLATE_CMD names '$id', which is not in the catalogue"; stray=1; }
done
[ "$stray" -eq 0 ] && ok "every TEMPLATE_CMD id exists in the catalogue"

echo
echo "§6 the three templates a command cannot fix carry no command"

# Adding one would be worse than the defect it appears to cure:
#   authentik  — needs PostgreSQL and Redis. With `server` it runs happily and
#                retries the database for ever: a container that reports healthy
#                and never serves, which is a SILENT failure where today's is loud.
#   vllm       — takes its model as a command-line argument, but TEMPLATE_CMD is a
#                static &[&str] and cannot interpolate the MODEL the template
#                collects.
#   surrealdb  — needs a datastore path AND a writable volume. It runs as
#                `nonroot` while the agent creates volume dirs root-owned, so it
#                cannot write /data; without the path it silently keeps everything
#                in memory and loses it on restart.
for id in authentik vllm surrealdb; do
  if grep -qF "(\"$id\"," <<<"$FLAT"; then
    bad "TEMPLATE_CMD declares '$id' — see this section's comment; that command cannot work"
  else
    ok "'$id' carries no command"
  fi
done

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
