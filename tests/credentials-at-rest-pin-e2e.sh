#!/usr/bin/env bash
# Regression pins for the s377 ship — the credential promise the project publishes.
#
# `SECURITY.md` and `README.md` both state, without qualification, that every
# stored credential is encrypted at rest with AES-256-GCM. Three columns were
# not: `cdn_zones.api_key`, `git_deploys.github_token` and `servers.agent_token`
# — the last one named in cleartext by `auto_healer`'s own comment about what a
# panel database dump carries. The claim was false on the project's front page,
# and no test could see it, because nothing compared the published sentence with
# the columns.
#
# So this suite pins BOTH ENDS. §A–§C pin each column's writer and its single
# decrypting read site; §D pins the masks on the wire; §E pins the sentence
# against the code, so the promise cannot come back without the columns, and the
# columns cannot regress without the promise going red.
#
# The choke-point shape is the load-bearing one and it is deliberate: every read
# of these three credentials funnels through exactly one function
# (`helpers::cf_headers`, `cdn::bunny_headers`, `git_deploys::set_github_status`,
# `AgentRegistry::for_server`), so no read site can forget to decrypt. Arms that
# count CALL sites rather than the helper's own name are used throughout —
# a name search is satisfied by the declaration itself, which is how a severed
# filter came back green at s376 (#592).
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0; FAIL=0
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
eq()  { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected '$3', got '$2'"; }

# ugrep's --ignore-files shim honours .gitignore, so every count below uses the
# real binary explicitly (dockpanel-ops-p2, s357).
G=/usr/bin/grep

CDN=panel/backend/src/routes/cdn.rs
GITD=panel/backend/src/routes/git_deploys.rs
SERVERS=panel/backend/src/routes/servers.rs
AGENT=panel/backend/src/services/agent.rs
HELPERS=panel/backend/src/helpers.rs
REENC=panel/backend/src/services/credential_reencrypt.rs

for f in "$CDN" "$GITD" "$SERVERS" "$AGENT" "$HELPERS" "$REENC" README.md SECURITY.md; do
  [ -f "$f" ] || { bad "SETUP" "$f missing"; exit 1; }
done

# An arm that measures an empty subject prints green for every absence below, so
# each subject is asserted before it is measured (lesson #143).
for pair in "$CDN:400" "$GITD:3000" "$SERVERS:400" "$AGENT:1200"; do
  f=${pair%:*}; floor=${pair##*:}
  n=$($G -c '' "$f")
  [ "$n" -gt "$floor" ] && ok "S0 subject extracted — $f is $n lines" \
    || bad "S0 subject extracted" "$f is only $n lines (floor $floor) — arms over it examine nothing"
done

echo "── A. cdn_zones.api_key ────────────────────────────────────────────────"

# A1: the WRITER binds the ciphertext variable, not the request field. Keyed on
# the bind, not on the presence of `encrypt_credential` anywhere in the file —
# the call could exist and the bind still carry `body.api_key`.
eq "A1 the create writer binds the encrypted value" \
   "$($G -c '\.bind(&api_key_enc)' "$CDN")" "1"
API_KEY_PLAIN=$($G -c '\.bind(body\.api_key' "$CDN")
if [ "$API_KEY_PLAIN" = "0" ]; then
  ok "A1b no writer binds the plaintext api_key"
else
  bad "A1b no writer binds the plaintext api_key" "found $API_KEY_PLAIN — the column would hold cleartext"
fi
eq "A1-control the same bind shape DOES match another column in this file" \
   "$([ "$($G -c '\.bind(body\.domain\.trim())' "$CDN")" -ge 1 ] && echo yes || echo no)" "yes"

# A2: the single decrypting choke point. Every Bunny caller funnels through
# `bunny_headers`, so the decrypt lives there rather than at each read site.
eq "A2 bunny_headers decrypts" \
   "$($G -c 'decrypt_credential_from_env(api_key)' "$CDN")" "1"
eq "A2b every Bunny read site goes through the choke point" \
   "$($G -c 'headers(bunny_headers(&zone\.api_key))' "$CDN")" "4"
eq "A2c the Cloudflare half already had its own choke point" \
   "$($G -c 'decrypt_credential_from_env(token)' "$HELPERS")" "1"

echo "── B. git_deploys.github_token ─────────────────────────────────────────"

eq "B1 both writers bind through the shared encrypting helper" \
   "$($G -c 'encrypt_stored_token(body\.github_token\.as_deref()' "$GITD")" "2"
GH_PLAIN=$($G -cE '\.bind\((&body\.github_token|body\.github_token\.as_deref\(\))\)' "$GITD")
if [ "$GH_PLAIN" = "0" ]; then
  ok "B1b no writer binds the plaintext github_token"
else
  bad "B1b no writer binds the plaintext github_token" "found $GH_PLAIN — the column would hold cleartext"
fi

# B2: the sentinel guard. Every handler now returns the masked row, so a client
# that submits the form it was served sends ●●●●●●●● back. Storing that would
# replace a working token with eight circles; encrypting it would be worse, since
# the result looks like a legitimate credential to every later read. v2.48.3
# shipped this class of bug once already.
eq "B2 the mask sentinel is refused by the writer-side helper" \
   "$($G -c 't == GITHUB_TOKEN_MASK' "$GITD")" "1"
eq "B2b the mask and the guard read the same constant" \
   "$($G -c 'GITHUB_TOKEN_MASK' "$GITD")" "3"

# B3: the single sink. All seven read sites reach `set_github_status`, directly
# or through a spawned task, and the token never leaves the backend.
eq "B3 the sink decrypts" \
   "$($G -c 'decrypt_credential_from_env(token)' "$GITD")" "1"
AGENT_SEES_TOKEN=$($G -rc 'github_token' panel/agent/src 2>/dev/null | $G -cv ':0$')
if [ "${AGENT_SEES_TOKEN:-0}" = "0" ]; then
  ok "B3b the github token never leaves the backend"
else
  bad "B3b the github token never leaves the backend" "the agent tree references it — the sink is not the only reader"
fi
eq "B3-control the agent tree IS being read (a token name that does appear there)" \
   "$([ "$($G -rc 'agent.token' panel/agent/src 2>/dev/null | $G -cv ':0$')" -ge 1 ] && echo yes || echo no)" "yes"

echo "── C. servers.agent_token ──────────────────────────────────────────────"

eq "C1 the create writer binds the encrypted value" \
   "$($G -c '\.bind(&agent_token_enc)' "$SERVERS")" "1"
eq "C1b the rotation writer binds the encrypted value" \
   "$($G -c '\.bind(&new_token_enc)' "$SERVERS")" "1"
eq "C1c both ensure_local_server writers bind the encrypted value" \
   "$($G -c '\.bind(&agent_token_enc)' "$AGENT")" "2"

# C2: the ONLY consumer of the stored dial-out token. The inbound direction is
# authenticated by `agent_token_hash` and is untouched by this ship — asserted
# here so a future reader does not "helpfully" encrypt the hash too.
eq "C2 for_server decrypts before dialling" \
   "$($G -c 'decrypt_credential_or_legacy(' "$AGENT")" "1"
eq "C2b the key is a struct field, not an environment read" \
   "$($G -c '&self\.jwt_secret' "$AGENT")" "1"
eq "C2c the inbound path still authenticates on the HASH" \
   "$($G -c 'hash_agent_token' panel/backend/src/routes/agent_checkin.rs)" "1"

# C3: legacy tolerance is what makes this migration-free. `_or_legacy` returns a
# pre-encryption plaintext token unchanged, so an existing fleet keeps dialling.
# If this ever became the strict `decrypt_credential`, every fleet member
# enrolled before this release would stop being reachable.
eq "C3 the dial-out decrypt is the legacy-tolerant variant" \
   "$($G -c 'decrypt_credential_or_legacy(\s*$\|decrypt_credential_or_legacy(' "$AGENT")" "1"

# C4 / the shared premise. Two of the three choke points decrypt with
# `decrypt_credential_from_env`, which reads JWT_SECRET from the process
# environment, while every WRITER encrypts with `state.config.jwt_secret`. Those
# must be the same value or every read returns ciphertext where a credential
# belongs — silently, since the legacy fallback returns the input verbatim. They
# are the same by construction, and this arm is what keeps them so: `Config`
# takes the field straight from the variable and the process refuses to start
# without it. `cf_headers` has depended on this since before the ship; two more
# credentials depend on it now.
eq "C4 config.jwt_secret IS the JWT_SECRET environment variable" \
   "$($G -c 'let jwt_secret = std::env::var("JWT_SECRET").expect(' panel/backend/src/config.rs)" "1"

echo "── D. The wire: every row-returning handler masks ──────────────────────"

# D1: DERIVED, not listed. The population is every handler that returns a
# GitDeploy row; each must mask before it answers. A hand list of handler names
# is the defect this ship is about, one level up.
RETURNS=$($G -cE '^\) -> Result<(\(StatusCode, )?Json<(Vec<)?GitDeploy>' "$GITD")
MASKS=$($G -c 'mask_github_token(' "$GITD")
# masks = one call per returning handler, plus the definition
eq "D1 every GitDeploy-returning handler masks" "$MASKS" "$((RETURNS + 1))"
[ "$RETURNS" -ge 4 ] && ok "D1-control the handler population is $RETURNS, not empty" \
  || bad "D1-control the handler population" "found $RETURNS — the signature grep matched nothing"

echo "── E. The published sentence and the columns are one edit ──────────────"

# E1: the claim. This is the arm that would have caught the original defect: the
# promise is unqualified on two surfaces, so every credential column must be in
# the sweep. It is pinned as a PRESENCE test on the sentence plus a membership
# test on the registry, because the two halves drifted apart silently for months.
eq "E1 README still publishes the at-rest claim" \
   "$($G -c 'Credentials are encrypted at rest with AES-256-GCM' README.md)" "1"
eq "E1b SECURITY.md still publishes it unqualified" \
   "$($G -c 'All stored credentials encrypted with AES-256-GCM' SECURITY.md)" "1"

# E2: the three columns this ship added are in the sweep's own registry. A
# module named without its subject is the half-edit `subject_tokens_match_the_sweep`
# rejects in Rust; this is the source-level mirror, so the pair cannot be
# separated by an edit that never runs cargo test.
for sub in '("cdn_zones", "id", "api_key")' '("git_deploys", "id", "github_token")' '("servers", "id", "agent_token")'; do
  # COUNT, never forbid: the file's own doc comment spells one of these
  # declarations in prose, so an absence test could not fail (s376 #386 family).
  n=$($G -cF "    $sub," "$REENC")
  eq "E2 the sweep re-keys $sub" "$n" "1"
done
eq "E2b each new column's module names it in COVERED_MODULES" \
   "$($G -cE '\("(cdn|git_deploys|servers|agent)", "(cdn_zones\.api_key|git_deploys\.github_token|servers\.agent_token)"\),' "$REENC")" "4"

# E3: the module list is DERIVED from the crate, not typed. Pinned because the
# earlier cut walked a two-element literal and asserted that literal against
# itself — a tautology a mutation walked straight through.
eq "E3 the writer census walks the crate root, not a directory list" \
   "$($G -c 'join("src");' "$REENC")" "1"
eq "E3b it descends" "$($G -c 'stack.push(path);' "$REENC")" "1"

echo
echo "PASS $PASS · FAIL $FAIL"
[ "$FAIL" -eq 0 ]
