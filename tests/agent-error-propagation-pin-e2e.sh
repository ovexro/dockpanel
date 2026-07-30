#!/usr/bin/env bash
#
# Pins the fix for issues #90, #91 and #92 — three separate reporters filing
# three separate bugs against one defect: the panel told them "Agent offline —
# the DockPanel agent is not responding" while the agent was answering
# perfectly clearly ("Path must be within /var/backups/ or /tmp/", "WAF not
# installed", a full nginx -t diagnostic).
#
# Two halves produced the lie and both are pinned here:
#   backend  — agent_error() took `impl Display`, so AgentError::Status(code,
#              body) was stringified and every non-2xx became one generic 502.
#   frontend — api.ts rewrote ANY 502 into the "agent offline" sentence.
#
# Plus the WAF half of #92: the installer's unicode.mapping fallback wrote a
# ZERO-BYTE file, which ModSecurity cannot parse, so nginx -t failed the first
# time a site enabled the WAF — after the installer had reported success.
#
# ⚠ Every source check below strips comments before searching. This file's own
# subject matter is spelled out in the comments of the code it inspects, so a
# raw-source grep would credit (or convict) the prose that explains the trap
# rather than the code that avoids it. See feedback_source_pin_prose_trap and
# lesson #110f — that trap has now fired on three separate suites.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0
FAIL=0
ok()  { PASS=$((PASS+1)); printf '  ✓ %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  ✗ %s\n' "$1"; }

ERR_RS=panel/backend/src/error.rs
API_TS=panel/frontend/src/api.ts
SVC_RS=panel/agent/src/routes/service_installer.rs
MIG_TSX=panel/frontend/src/pages/Migration.tsx
MAPPING=panel/agent/assets/unicode.mapping

# Strip block comments, then whole-line `//` comments. `//` is only stripped at
# the start of a (trimmed) line: stripping it mid-line would blank the inside
# of every string containing a URL and fail in the direction that PASSES.
code_of() {
  sed 's:/\*.*\*/::g' "$1" | awk '
    /\/\*/ { inblk=1 }
    !inblk { t=$0; sub(/^[ \t]+/,"",t); if (t !~ /^\/\//) print }
    /\*\// { inblk=0 }
  '
}

# grep -c, never grep -q: under pipefail a `grep -q` match SIGPIPEs its
# producer and the pipeline reports FAILURE for a SUCCESSFUL match (#103d).
has() { [ "$(code_of "$1" | grep -cF -- "$2")" -gt 0 ]; }
hasre() { [ "$(code_of "$1" | grep -cE -- "$2")" -gt 0 ]; }

echo "§1 the status survives the boundary"

if hasre "$ERR_RS" 'pub fn agent_error\(context: &str, e: AgentError\)'; then
  ok "agent_error takes AgentError by value, so the status cannot be stringified away"
else
  bad "agent_error no longer takes AgentError — an 'impl Display' bound is exactly what let 244 call sites discard the agent's status code"
fi

if hasre "$ERR_RS" '\(400\.\.500\)\.contains\(&code\)'; then
  ok "a 4xx from the agent is handled as its own case"
else
  bad "no 4xx branch in agent_error — agent refusals are being collapsed again"
fi

if hasre "$ERR_RS" 'StatusCode::from_u16\(code\)'; then
  ok "the agent's own status is what gets returned, not a fixed one"
else
  bad "agent_error does not rebuild the status from the agent's code"
fi

echo
echo "§2 only an unreachable agent is reported as unreachable"

if hasre "$ERR_RS" 'AgentError::Connection\(_\) \| AgentError::CircuitOpen\(_\)'; then
  ok "Connection and CircuitOpen are the arms that carry the unreachable marker"
else
  bad "the unreachable marker is no longer scoped to transport failures"
fi

# The marker must NOT be reachable from the Status arms. Count its occurrences
# in PRODUCTION code: one definition + one use in the transport arm is the whole
# story. The unit tests below assert on it too, and a scanner that cannot tell
# a guard from a test convicts the test (#94c).
MARKER_USES=$(code_of "$ERR_RS" | sed '/#\[cfg(test)\]/q' | grep -cF 'CODE_AGENT_UNREACHABLE')
if [ "$MARKER_USES" -eq 2 ]; then
  ok "CODE_AGENT_UNREACHABLE appears exactly twice in production code (definition + the one arm that earns it)"
else
  bad "CODE_AGENT_UNREACHABLE appears $MARKER_USES times in production code, expected 2 — a Status or Parse arm may now be claiming the agent is offline"
fi

if hasre "$ERR_RS" 'v\.get\("error"\)' && hasre "$ERR_RS" 'or_else\(\|\| v\.get\("message"\)\)'; then
  ok "both agent error-body shapes are understood ({error:…} and {success:false,message:…})"
else
  bad "only one agent error-body shape is parsed — the other renders as raw JSON to the user"
fi

echo
echo "§3 a 5xx still hides its internals"

if hasre "$ERR_RS" 'Operation failed\. Reference:'; then
  ok "agent 5xx keeps the generic incident-id response"
else
  bad "the 5xx path no longer returns a generic message — internals may leak"
fi

echo
echo "§4 the frontend stops inferring a cause from a status code"

if hasre "$API_TS" 'code\?: string \}\)\.code === "agent_unreachable"'; then
  ok "api.ts branches on the backend's marker"
else
  bad "api.ts does not branch on the agent_unreachable marker"
fi

# NEGATIVE: the blanket rewrite must be gone from the CODE. The comment above
# it describes the old behaviour on purpose, which is why this reads code only.
if hasre "$API_TS" 'res\.status === 502'; then
  bad "api.ts still keys the 'agent offline' message off a bare 502 — that is the defect in #90/#91/#92"
else
  ok "no bare-502 branch remains in api.ts"
fi

echo
echo "§5 third-party failures are not the agent's fault"

if hasre "$ERR_RS" 'pub fn upstream_error'; then
  ok "upstream_error exists for third-party calls"
else
  bad "no upstream_error — Stripe/Cloudflare failures have nowhere honest to go"
fi

# billing.rs talks ONLY to Stripe — it makes no agent call at all, so any
# agent_error in it is by definition mislabelling a third-party failure.
n=$(code_of panel/backend/src/routes/billing.rs | grep -cF 'agent_error(')
if [ "$n" -eq 0 ]; then
  ok "billing.rs routes no Stripe error through agent_error"
else
  bad "billing.rs calls agent_error $n time(s) — it makes no agent calls, so a Stripe outage reports the agent as offline"
fi

# dns.rs legitimately talks to BOTH: Cloudflare over reqwest and the agent for
# the cloudflared service. The separation is type-enforced (agent_error only
# accepts AgentError, which a reqwest::Error is not), so what is pinned here is
# that the Cloudflare half has somewhere honest to go.
if hasre panel/backend/src/routes/dns.rs 'upstream_error\("Cloudflare'; then
  ok "dns.rs reports Cloudflare failures as Cloudflare failures"
else
  bad "dns.rs no longer routes Cloudflare failures through upstream_error"
fi

echo
echo "§6 the WAF's unicode.mapping cannot be empty (#92)"

if [ -f "$MAPPING" ]; then
  ok "the vendored mapping asset is in the tree"
else
  bad "panel/agent/assets/unicode.mapping is missing — the include_str! will not compile"
fi

SZ=$(wc -c < "$MAPPING" 2>/dev/null || echo 0)
if [ "$SZ" -gt 40000 ]; then
  ok "the mapping asset is a real file ($SZ bytes)"
else
  bad "the mapping asset is $SZ bytes — a truncated mapping fails nginx -t exactly like the empty one did"
fi

if [ "$(grep -cE '^20127' "$MAPPING" 2>/dev/null || echo 0)" -gt 0 ]; then
  ok "the asset defines code page 20127, which modsecurity.conf names"
else
  bad "the asset has no 20127 section — the code page modsecurity.conf asks for is absent"
fi

if hasre "$SVC_RS" 'include_str!\("\.\./\.\./assets/unicode\.mapping"\)'; then
  ok "the mapping rides the binary instead of depending on distro packaging"
else
  bad "unicode.mapping is not embedded — Debian 13 ships no copy, so the install writes nothing usable"
fi

# NEGATIVE: the zero-byte fallback. Written as a regex over CODE so the comment
# explaining why it was removed cannot satisfy it.
if hasre "$SVC_RS" 'write\("/etc/modsecurity/unicode\.mapping", ""\)'; then
  bad "the empty-file fallback is back — it creates a file ModSecurity cannot parse and reports the install as successful"
else
  ok "no zero-byte unicode.mapping fallback"
fi

if hasre "$SVC_RS" 'c\.contains\("20127"\)'; then
  ok "the installer verifies the mapping it just wrote, rather than assuming it"
else
  bad "the installer does not check unicode.mapping after writing — #45's shape, and the reason #92 shipped as a success"
fi

echo
echo "§7 the migration field asks for a path the agent accepts (#90)"

if hasre "$MIG_TSX" 'placeholder="/var/backups/'; then
  ok "the placeholder suggests an accepted directory"
else
  bad "the Backup File Path placeholder does not suggest /var/backups/"
fi

if hasre "$MIG_TSX" 'placeholder="/home/'; then
  bad "the placeholder still suggests /home/, which routes/migration.rs is written to refuse"
else
  ok "the placeholder no longer suggests a path the agent rejects"
fi

if has "$MIG_TSX" '/var/backups/' && has "$MIG_TSX" '/tmp/'; then
  ok "the field states the real constraint to the operator"
else
  bad "the helper text does not state where the archive must live"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
