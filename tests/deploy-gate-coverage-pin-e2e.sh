#!/usr/bin/env bash
# deploy-gate-coverage-pin-e2e.sh — s369
#
# THE GUARD HAS TO COVER THE CLASS, NOT THE DOOR IT WAS WRITTEN FOR.
#
# `image_scans::preflight_gate` is the CVE deploy gate. Until v2.122.0 exactly
# one handler called it — the template deploy — while `SECURITY.md`, the README
# and the Settings toggle all said "deploys" without qualification. Changing an
# app's image, deploying a Compose file, and creating or editing a stack each
# put an operator-chosen image on a host and asked the threshold nothing.
#
# The same was true of `container_policies.allowed_images`, whose own help text
# says "Restrict which Docker images a user can deploy" and which was read by
# that one handler only — and compared against the catalogue id rather than an
# image.
#
# So this suite does NOT list the doors that were fixed. That would be an
# instance arm wearing a class arm's comment (lesson #545 — v2.120.0's foreign
# key arm named `users` and could not see the two bare keys pointing at
# `servers`). It DERIVES the door set:
#
#   A = agent service functions that call `docker.create_container(`
#   B = agent route handlers that call something in A
#   C = the agent route PATHS bound to the handlers in B
#   D = backend handlers that post to a path in C
#
# and asserts that every member of D consults the gate, or is a NAMED exemption
# whose reason is written in the source. A new door joins D by existing.
#
# ⚠ Never put the row loop on the right-hand side of a pipe: the loop runs in a
# subshell and `ok`/`bad` increment counters that die at the closing brace, so
# the suite prints marks and reports a stale PASS (lesson #548).

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  deploy-gate coverage — source pins (s369)"
echo "=============================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

G=/usr/bin/grep

AGENT_SVC=panel/agent/src/services
AGENT_ROUTES=panel/agent/src/routes/docker_apps.rs
BE_ROUTES=panel/backend/src/routes
SCANS=$BE_ROUTES/image_scans.rs

for f in "$AGENT_ROUTES" "$SCANS" "$BE_ROUTES/docker_apps.rs" "$BE_ROUTES/stacks.rs" "$BE_ROUTES/mod.rs"; do
  [ -f "$f" ] || { echo "  FATAL: $f missing — wrong tree?"; exit 1; }
done

# ── §0 discovery, with floors ───────────────────────────────────────────────
# A discovery that silently returns nothing makes every arm below pass by
# looking at zero subjects (lesson #143).
echo "── §0 discovering the doors ──"

# A: agent service functions that create a container.
CREATORS=$($G -rl "create_container(" "$AGENT_SVC" --include=*.rs | sort -u)
CREATORS_N=$(printf '%s\n' "$CREATORS" | $G -c . || true)
if [ "$CREATORS_N" -ge 2 ]; then
  ok "found $CREATORS_N agent service file(s) that create containers (>= 2 known to exist)"
else
  bad "only $CREATORS_N agent service file(s) create containers — the scan broke; every arm below is vacuous"
fi

# C: the agent route paths whose handlers reach those services. Read off the
# agent's own route table so a renamed or added route is picked up.
# Anchored at the end: `/apps/update-check` and `/apps/{id}/update-limits` are
# routes that never create a container, and a loose `update` match would drag
# them in and then report them ungated for ever.
AGENT_PATHS=$($G -oE '\.route\("(/apps[^"]*)"' "$AGENT_ROUTES" \
              | $G -oE '/apps[^"]*' \
              | $G -E '(deploy|change-image|/update)$' | sort -u)
AGENT_PATHS_N=$(printf '%s\n' "$AGENT_PATHS" | $G -c . || true)
if [ "$AGENT_PATHS_N" -ge 3 ]; then
  ok "found $AGENT_PATHS_N container-creating agent route(s) (>= 3 known to exist)"
else
  bad "only $AGENT_PATHS_N container-creating agent route(s) — the route-table scan broke"
fi
printf '     %s\n' $AGENT_PATHS

# D: backend handlers that post to one of those paths. Matched on the literal
# prefix, because the backend builds them with format! holes.
DOORS=()
PATHS_F=$(mktemp); DOORS_F=$(mktemp)
trap 'rm -f "$PATHS_F" "$DOORS_F"' EXIT
printf '%s\n' $AGENT_PATHS > "$PATHS_F"

# The finder runs from a quoted heredoc, never from `python3 -c '...'` inside
# the shell: a regex full of quotes and braces does not survive that trip, and
# the failure is silent — the pattern arrives mangled, matches nothing, and the
# arm below reports a clean tree (lesson #547).
python3 - "$PATHS_F" > "$DOORS_F" <<'PYEOF'
import re, sys, glob
paths = [l.strip() for l in open(sys.argv[1]) if l.strip()]

def to_pattern(p):
    # A backend caller builds these with format! holes, so match the SHAPE.
    # Anchored on the closing quote so ".../update" does not also match
    # ".../update-limits", which creates no container.
    body = re.sub(r"\{[^}]*\}", "\x00", p)
    return re.compile(re.escape(body).replace("\x00", '[^"]*') + '"')

pats = [to_pattern(p) for p in paths]
for f in sorted(glob.glob("panel/backend/src/routes/*.rs")):
    src = re.sub(r"^\s*//.*$", "", open(f).read(), flags=re.M)  # prose is not a door (#149)
    fn = None
    for line in src.split("\n"):
        m = re.match(r"pub async fn (\w+)\(", line)
        if m:
            fn = m.group(1)
            continue
        if fn and any(p.search(line) for p in pats):
            print(f + "|" + fn)
            fn = None
PYEOF
while IFS='|' read -r file fn; do
  [ -n "$file" ] && DOORS+=("$file|$fn")
done < <(sort -u "$DOORS_F")
DOORS_N=${#DOORS[@]}
if [ "$DOORS_N" -ge 4 ]; then
  ok "found $DOORS_N backend door(s) that put an image on a host (>= 4 known to exist)"
else
  bad "only $DOORS_N backend door(s) discovered — the caller scan broke; §1 is vacuous"
fi
for d in "${DOORS[@]}"; do printf '     %s\n' "${d/|/ :: }"; done

# The one door that must NOT gate, and why. An exemption is a product decision,
# so it has to be written down in the source it exempts — this arm checks that
# the reason is there, not merely that the name is on a list.
EXEMPT_FN="update_app"
EXEMPT_WHY="re-pulls the SAME reference"

# ── §1 every door consults the gate ─────────────────────────────────────────
echo
echo "── §1 every container-creating door consults the CVE deploy gate ──"
UNGATED=""
GATED_N=0

# A door may reach the gate through a wrapper — `git_deploys` does, because two
# background tasks needed the same three checks. An arm that greps for the
# gate's own name would call those doors ungated the moment someone extracts a
# helper (lesson #150: extracting a helper moves the subject of every pin that
# measured it). So build the set of functions that REACH the gate, one hop.
GATE_REACHING="preflight_gate|preflight_gate_image"
while IFS= read -r wrapper; do
  [ -n "$wrapper" ] && GATE_REACHING="$GATE_REACHING|$wrapper"
done < <(
  $G -rhB 40 -E 'preflight_gate(_image)?\(' $BE_ROUTES/*.rs \
    | $G -oE '^(pub )?(async )?fn [a-z_]+' | $G -oE '[a-z_]+$' | sort -u \
    | $G -vE '^(preflight_gate|preflight_gate_image)$'
)

for d in "${DOORS[@]}"; do
  file=${d%%|*}; fn=${d##*|}
  body=$(perl -0777 -ne '
      if (/pub async fn \Q'"$fn"'\E\(.*?(?=\npub (?:async )?fn |\z)/s) { print $& }
    ' "$file")
  if [ -z "$body" ]; then
    bad "could not extract $fn from $file — the extractor broke, not the code"
    continue
  fi
  if $G -qE "($GATE_REACHING)\(" <<< "$body"; then
    GATED_N=$((GATED_N+1))
  elif [ "$fn" = "$EXEMPT_FN" ]; then
    :
  else
    UNGATED="$UNGATED ${file##*/}::$fn"
  fi
done
if [ -z "$UNGATED" ]; then
  ok "all $GATED_N non-exempt doors call preflight_gate/preflight_gate_image"
else
  bad "door(s) that run an operator-chosen image WITHOUT asking the gate —$UNGATED — the threshold an operator set does not apply there (#545)"
fi

# Exact, not >=: an arm that asks for "at least one" survives deleting one
# (lesson #546).
EXPECTED_GATED=7
if [ "$GATED_N" -eq "$EXPECTED_GATED" ]; then
  ok "exactly $EXPECTED_GATED doors call the gate, as expected"
else
  bad "expected $EXPECTED_GATED gated doors, found $GATED_N — a door was added or a call removed; update this number deliberately"
fi

# ── §2 the exemption is justified in the source it exempts ──────────────────
echo
echo "── §2 the one exemption states its reason where it applies ──"
GATE_DOC=$(perl -0777 -ne 'print $& if /\/\/\/ The gate itself.*?pub async fn preflight_gate_image/s' "$SCANS")
if [ -z "$GATE_DOC" ]; then
  bad "could not read preflight_gate_image's doc block — §2 is vacuous"
elif $G -qF "$EXEMPT_WHY" <<< "$GATE_DOC" && $G -qF "$EXEMPT_FN" <<< "$GATE_DOC"; then
  ok "the doc names $EXEMPT_FN and why it is exempt (it re-pulls the same reference)"
else
  bad "preflight_gate_image no longer explains why $EXEMPT_FN is exempt — an undocumented exemption is indistinguishable from a missed door"
fi

# ── §3 allowed_images is compared against an IMAGE, not a catalogue id ──────
echo
echo "── §3 the allowed-images policy judges an image ──"
DEPLOY_BODY=$(perl -0777 -ne 'print $& if /pub async fn deploy\(.*?(?=\npub (?:async )?fn )/s' "$BE_ROUTES/docker_apps.rs")
if [ -z "$DEPLOY_BODY" ]; then
  bad "could not extract deploy() — §3 is vacuous"
else
  if $G -qE 'body\.template_id\.contains\(' <<< "$DEPLOY_BODY"; then
    bad "allowed_images is matched with template_id.contains() again — the field says images and a substring test admits evil/nginx-backdoor"
  else
    ok "allowed_images is no longer a substring test over the catalogue id"
  fi
  if $G -qE 'image_allowed_by\(' <<< "$DEPLOY_BODY"; then
    ok "deploy() judges the resolved image through image_allowed_by"
  else
    bad "deploy() no longer calls image_allowed_by — the policy stopped reading the image"
  fi
fi

POLICY_N=$($G -c "enforce_allowed_images(" $BE_ROUTES/docker_apps.rs $BE_ROUTES/stacks.rs 2>/dev/null | awk -F: '{s+=$2} END {print s+0}')
# 1 definition + 4 call sites.
if [ "$POLICY_N" -eq 5 ]; then
  ok "enforce_allowed_images is defined once and called at all 4 image-taking doors"
else
  bad "expected 5 enforce_allowed_images occurrences (1 def + 4 doors), found $POLICY_N"
fi

# ── §4 the published claim does not outrun the code ─────────────────────────
echo
echo "── §4 what SECURITY.md and the README promise is what the code does ──"
SEC_LINE=$($G -n "soft deploy gate" SECURITY.md | head -1)
if [ -z "$SEC_LINE" ]; then
  bad "SECURITY.md no longer describes the deploy gate — check what replaced it"
elif $G -qE "every door|change-image|changing an app" <<< "$SEC_LINE"; then
  ok "SECURITY.md names the doors the gate covers rather than saying 'deploys'"
else
  bad "SECURITY.md claims the gate refuses 'deploys' without naming which doors — it did that while covering one of six"
fi
if $G -qE "template deploys, image changes, compose deploys and stacks" README.md; then
  ok "README enumerates the covered doors"
else
  bad "README no longer enumerates the covered doors"
fi
if $G -qE "Updating an app is deliberately exempt" docs/guides/image-scanning.md; then
  ok "the guide discloses the update exemption"
else
  bad "the guide no longer discloses that Update is exempt — the one thing an operator would otherwise assume is covered"
fi

# ── §5 compose images are parsed, not grepped ───────────────────────────────
echo
echo "── §5 the gate's input comes from a parse ──"
CI=$(perl -0777 -ne 'print $& if /pub fn compose_images\(.*?\n\}/s' "$BE_ROUTES/mod.rs")
if [ -z "$CI" ]; then
  bad "compose_images is gone — the compose doors have nothing to gate on"
elif $G -qF 'serde_yaml_ng::from_str' <<< "$CI"; then
  ok "compose_images deserialises the document (a string scan is bypassable with anchors)"
else
  bad "compose_images no longer parses the YAML — anchors and aliases would hide an image from the gate"
fi

# ── §6 the buttons that PRODUCE the scans pass a name, not a container id ───
#
# `/api/apps/{name}/scan` and `/api/apps/{name}/sbom` resolve the image by
# matching the agent's `name` field. Every other `/api/apps/{container_id}/…`
# route takes the hex id, and `Apps.tsx` passed that hex id to these two — so
# "Rescan now", "Download SBOM" and the empty-state "Scan now" answered 404 for
# every app from 2026-04-15 until v2.122.0. The gate reads the rows these
# buttons write, so a gate with no way to produce a scan is a gate with nothing
# to read.
echo
echo "── §6 the app-keyed scan surface is keyed by NAME ──"
APPS_TSX=panel/frontend/src/pages/Apps.tsx
BAD_KEY=$($G -c "rescanApp(matchedApp.container_id\|downloadSbom(matchedApp.container_id" "$APPS_TSX" || true)
GOOD_KEY=$($G -c "rescanApp(matchedApp.name\|downloadSbom(matchedApp.name" "$APPS_TSX" || true)
if [ "$BAD_KEY" -eq 0 ] && [ "$GOOD_KEY" -eq 3 ]; then
  ok "all 3 scan/SBOM buttons pass the app NAME to a name-keyed route"
else
  bad "scan/SBOM buttons: $GOOD_KEY pass a name (expected 3), $BAD_KEY pass a container id (expected 0) — a hex id matches no app name and the route 404s"
fi
# Control: the hex id is still used where it belongs, so the grep above can fire
# in both directions rather than being green because the file changed shape.
if [ "$($G -c 'matchedApp.container_id' "$APPS_TSX" || true)" -ge 0 ] \
   && [ "$($G -c 'container_id' "$APPS_TSX" || true)" -ge 5 ]; then
  ok "control: container_id is still present in Apps.tsx, so the arm above is not green by absence"
else
  bad "control: container_id has vanished from Apps.tsx — §6 may be green because the file changed shape, not because it is correct"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
