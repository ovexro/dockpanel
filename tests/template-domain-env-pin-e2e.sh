#!/usr/bin/env bash
# Regression pins for the s363 fix to GitHub #111 — a template's URL-shaped env
# vars are filled in from the domain the operator claimed, and the Apps env
# editor can compose an arbitrary key.
#
# The DERIVATION itself is covered by mutation-proved Rust unit tests in
# panel/agent/src/services/docker_apps.rs. This suite pins the things a unit
# test cannot reach: the frontend affordance, the entry gate, the validation
# parity on the door the new affordance makes reachable, and the both-ends
# payload contract between the React client and the agent DTO (the client cast
# is unvalidated, so a key rename is invisible to tsc).
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0; FAIL=0
ok()   { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad()  { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
eq()   { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected '$3', got '$2'"; }

# ugrep's --ignore-files shim honours .gitignore, so every count below uses the
# real binary explicitly (dockpanel-ops-p2, s357).
G=/usr/bin/grep

APPS=panel/frontend/src/pages/Apps.tsx
AGENT=panel/agent/src/services/docker_apps.rs
AGENT_ROUTES=panel/agent/src/routes/docker_apps.rs
API=panel/backend/src/routes/docker_apps.rs

for f in "$APPS" "$AGENT" "$AGENT_ROUTES" "$API"; do
  [ -f "$f" ] || { bad "SETUP" "$f missing"; exit 1; }
done

echo "── A. The catalogue mechanism ─────────────────────────────────────────"

# A1 is an ENUMERATION arm, so it asserts its own subject list first: a census
# that silently reads zero rows prints green (lesson #143).
# Matches the tuple opener at top-level indent in BOTH shapes rustfmt produces —
# inline `("ghost", &[...])` and the split form whose id is on the next line.
# Inner pairs are indented deeper, so they cannot inflate this.
ENTRIES=$($G -c '^    (' <<<"$(sed -n '/^static TEMPLATE_DOMAIN_ENV/,/^];/p' "$AGENT")")
[ "${ENTRIES:-0}" -ge 6 ] && ok "A1 TEMPLATE_DOMAIN_ENV enumerates $ENTRIES templates" \
  || bad "A1" "expected >=6 keyed templates, extracted $ENTRIES — the window read nothing"

# A2: n8n is the reason the derived-append loop exists. If its row ever declares
# env vars, the vars stop flowing through that branch and the unit test that
# asserts the branch would be testing a path production no longer takes.
N8N_EMPTY=$(sed -n '/^        id: "n8n",$/,/^    },$/p' "$AGENT" | $G -c 'env_vars: &\[\],')
eq "A2 n8n still declares no catalogue env vars" "$N8N_EMPTY" "1"

# A3: the four names n8n needs, each present exactly once in the keyed list.
for v in N8N_HOST N8N_PROTOCOL WEBHOOK_URL N8N_PROXY_HOPS; do
  N=$(sed -n '/^static TEMPLATE_DOMAIN_ENV/,/^];/p' "$AGENT" | $G -c "\"$v\"")
  eq "A3 $v keyed once" "$N" "1"
done

# A4: a derived value must never carry the container port — through nginx the
# public URL has none, and shipping one recreates the very defect being fixed.
PORTED=$(sed -n '/^fn domain_env_for/,/^}/p' "$AGENT" | $G -cE 'https://\{domain\}:[0-9]')
eq "A4 no derived URL template carries a port" "$PORTED" "0"
# POSITIVE CONTROL for A4: the same shape must match a real ported literal
# elsewhere, or the zero above proves only that the pattern never runs.
CTL=$($G -cE 'http://localhost:[0-9]+' "$AGENT")
[ "$CTL" -gt 0 ] && ok "A4-control ported-URL pattern matches $CTL real literals" \
  || bad "A4-control" "pattern matched nothing anywhere — A4's zero is vacuous"

echo "── B. The deploy path actually consults it ────────────────────────────"

# B1: severed-pair guard. A keyed list nothing reads is the
# GPU_RECOMMENDED_TEMPLATES / stable-diffusion-webui shape, which sat unnoticed
# for 19 releases.
eq "B1 deploy_app derives from the claimed domain" \
   "$(sed -n '/^pub async fn deploy_app/,/^}/p' "$AGENT" | $G -c 'domain_env_for')" "1"

# B2: the merge helper is reached by deploy_app rather than orphaned by a later
# refactor (the #150 shape — extracting a helper moves the subject).
eq "B2 deploy_app builds its env through the merge helper" \
   "$(sed -n '/^pub async fn deploy_app/,/^}/p' "$AGENT" | $G -c 'merge_deploy_env(')" "1"

# B3: the flag the deploy form reads is emitted by the converter.
eq "B3 the serialized template carries the derived flag" \
   "$($G -c 'domain_derived: is_domain_derived(' "$AGENT")" "1"

echo "── C. The editor can compose a key (the actual #111 gap) ──────────────"

# C1: an add-row control exists in the Apps env editor.
eq "C1 Apps env editor offers Add variable" \
   "$($G -c 'setEnvEditRows(\[\.\.\.envEditRows, { key: "", value: "" }\])' "$APPS")" "1"

# C2: the key is an INPUT, not the read-only div it used to be. Keyed on the
# operation's syntax rather than a token that also appears in prose (lesson #149).
eq "C2 the key is editable" \
   "$($G -c 'n\[i\] = { \.\.\.n\[i\], key: e\.target\.value };' "$APPS")" "1"

# C3: a row can be removed.
eq "C3 a variable can be removed" \
   "$($G -c 'setEnvEditRows(envEditRows\.filter((_, j) => j !== i))' "$APPS")" "1"

# C4 ABSENCE: the Edit button must not be gated on the container already having
# variables — that gate is half of why a key could never be added.
GATED=$($G -c '!envEditing && envVars\.length > 0' "$APPS")
eq "C4 Edit is not gated on existing variables" "$GATED" "0"
# POSITIVE CONTROL for C4: the emptiness test still exists for the READ view,
# so the pattern shape is live and the zero above is a real zero.
EMPTYVIEW=$($G -c 'envVars\.length === 0' "$APPS")
[ "$EMPTYVIEW" -ge 1 ] && ok "C4-control the empty-state branch is still present ($EMPTYVIEW)" \
  || bad "C4-control" "no emptiness test anywhere — C4's zero may be vacuous"

# C5: a blank key never reaches the wire.
eq "C5 blank rows are dropped before sending" \
   "$($G -c 'envEditRows\.filter(r => r\.key\.trim() !== "")' "$APPS")" "1"

# C6: duplicates are refused rather than silently collapsed onto the last value.
eq "C6 duplicate names are refused" "$($G -c 'Duplicate variable name' "$APPS")" "1"

echo "── D. Validation parity on the door this makes reachable ──────────────"

# D1..D3: the PUT /env door had none of these until the editor could compose an
# arbitrary key. Asserted on the handler's own body, not the file, so the
# deploy door's identical checks cannot satisfy them.
ENVFN=$(sed -n '/^pub async fn update_env(/,/^}/p' "$API")
eq "D1 PUT /env caps the variable count"  "$($G -c 'Too many environment variables' <<<"$ENVFN")" "1"
eq "D2 PUT /env caps the value size"      "$($G -c 'Environment variable value too large' <<<"$ENVFN")" "1"
eq "D3 PUT /env refuses a name with '='"  "$($G -c "may not contain" <<<"$ENVFN")" "1"

echo "── E. Both-ends payload contract ──────────────────────────────────────"

# The client casts the response and posts a bare object, so neither tsc nor
# serde catches a key rename on either side. Pin the wire name at both ends.
eq "E1 client posts the env under its wire name" \
   "$($G -c 'api\.put(`/apps/\${envTarget}/env`, { env: payload })' "$APPS")" "1"
eq "E2 agent DTO reads the same wire name" \
   "$(sed -n '/^struct UpdateEnvRequest/,/^}/p' "$AGENT_ROUTES" | $G -c 'env: HashMap<String, String>')" "1"

echo
echo "PASS $PASS · FAIL $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
