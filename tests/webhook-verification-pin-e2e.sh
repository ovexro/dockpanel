#!/usr/bin/env bash
# Regression pins for the s374 ship — the webhook gateway's verification told the
# operator four things that were not true.
#
# 1. An endpoint could name an HMAC mode with no secret. The two verification
#    arms were `if let (Some, Some) … else { None }` and the rejection fired only
#    on `Some(false)`, so `None` passed — and the body was then relayed to every
#    enabled route. The endpoint list showed the mode, so it read as verified.
# 2. Worse than the null: the form posted "" for a field left blank, and HMAC
#    accepts a zero-length key. An endpoint with a blank secret and a real header
#    verified against the empty key, so anyone who guessed that produced a
#    signature recorded as AUTHENTIC and forwarded.
# 3. The rejection returned above the delivery INSERT, so the FALSE state of
#    `signature_valid` had no writer at all: the list's red badge could never
#    render and the guide's promise that failed verifications are logged was
#    false. §C pins the ORDER, which is the only thing that made it true.
# 4. The struct was returned to the browser with the shared secret still on it.
#
# The arms are shaped against the WIDENED form, not against deletion: every one
# of these defects was a value passing a check that existed, not a check that had
# been removed (lesson #423).
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0; FAIL=0
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
eq()  { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected '$3', got '$2'"; }

# ugrep's --ignore-files shim honours .gitignore, so every count below uses the
# real binary explicitly (dockpanel-ops-p2, s357).
G=/usr/bin/grep

GW=panel/backend/src/routes/webhook_gateway.rs
UI=panel/frontend/src/pages/WebhookGateway.tsx
GUIDE=docs/guides/webhook-gateway.md
ROUTES=panel/backend/src/routes/mod.rs

for f in "$GW" "$UI" "$GUIDE" "$ROUTES"; do
  [ -f "$f" ] || { bad "SETUP" "$f missing"; exit 1; }
done

# An arm that measures an empty subject prints green for every absence below, so
# the subjects are asserted before they are measured (lesson #143).
GW_LINES=$($G -c '' "$GW"); UI_LINES=$($G -c '' "$UI")
[ "$GW_LINES" -gt 400 ] && ok "A0 subject extracted — $GW is $GW_LINES lines" \
  || bad "A0 subject extracted" "$GW is only $GW_LINES lines — every arm below examined nothing"
[ "$UI_LINES" -gt 200 ] && ok "A0b subject extracted — $UI is $UI_LINES lines" \
  || bad "A0b subject extracted" "$UI is only $UI_LINES lines"

echo "── A. A verification mode cannot be stored without the means to perform it ─"

# A1: the presence check itself. Both halves are required, because a secret with
# no header and a header with no secret are equally unverifiable.
eq "A1 create refuses a verifying mode missing either half" \
   "$($G -cE 'verify_mode != "none" && \(verify_secret\.is_none\(\) \|\| verify_header\.is_none\(\)\)' "$GW")" "1"

# A2: blank is absent — two halves on the write side, two on the read side.
# Without this the check above passes on the exact input the form used to post,
# and the grader still hands a zero-length key to the HMAC. Counting all four
# means removing the filter from either path turns this red.
#
# Keyed on the FILTER alone, not on the whole `map(...).filter(...)` chain:
# rustfmt's chain_width wraps the two create-side calls across lines, and a
# line-based grep for a chain that spans lines can never fire (#409). The filter
# is the operation that does the work, so it is the honest thing to count.
eq "A2 a blank secret and a blank header are absent, on both the write and read paths" \
   "$($G -cE 'filter\(\|s\| !s\.is_empty\(\)\)' "$GW")" "4"
eq "A2-control both halves are trimmed before being judged empty" \
   "$($G -c 'map(str::trim)' "$GW")" "4"

# A3: ABSENCE arm with a positive control — the INSERT must bind the filtered
# locals, never the raw request fields, or A2 is decorative.
RAW_BINDS=$($G -cE '\.bind\(&req\.verify_(secret|header)\)' "$GW")
if [ "$RAW_BINDS" = "0" ]; then
  ok "A3 the endpoint INSERT does not bind the unfiltered request fields"
else
  bad "A3 the endpoint INSERT does not bind the unfiltered request fields" \
      "found $RAW_BINDS — a blank field would be stored as a present secret"
fi
eq "A3-control the same pattern shape DOES match a field that is bound raw" \
   "$($G -cE '\.bind\(&req\.description\)' "$GW")" "1"

echo "── B. The receive path fails CLOSED on anything it cannot vouch for ─────"

# B1: the shape that failed open is gone. Its tell was a verification arm whose
# else-branch produced the same value as "no verification configured".
OLD_ARM=$($G -cE 'if let \(Some\(secret\), Some\(header_name\)\)' "$GW")
if [ "$OLD_ARM" = "0" ]; then
  ok "B1 no verification arm collapses a missing secret into the not-configured value"
else
  bad "B1 no verification arm collapses a missing secret into the not-configured value" \
      "found $OLD_ARM — a missing secret is indistinguishable from no verification"
fi
# The control used to point at the inbound loop's own `if let (Some(path),
# Some(value))`, which moved into `route_admits` and became a let-else when the
# two forwarding paths were made to share one filter decision. Repointed at the
# minimal pair instead: B1's subject line itself, which now refuses via let-else.
# It differs from B1's forbidden pattern by the `if ` prefix ALONE, so it matches
# in both worlds — which is exactly what a control has to do.
eq "B1-control the same tuple destructure is still greppable in this file" \
   "$($G -cE 'let \(Some\(secret\), Some\(header_name\)\)' "$GW")" "1"

# B2/B3: the two mappings that decide whether an unverifiable delivery is
# recorded as attested and whether it is accepted. Pinned separately because a
# fix to one without the other reproduces half the defect.
eq "B2 an unverifiable delivery is recorded as NOT verified" \
   "$($G -cE 'SignatureVerdict::Invalid \| SignatureVerdict::Unverifiable => Some\(false\)' "$GW")" "1"
eq "B3 only an unconfigured or valid delivery is accepted" \
   "$($G -cE 'SignatureVerdict::NotConfigured \| SignatureVerdict::Valid => None' "$GW")" "1"

# B4: the failure default must not be the safe-looking one (lesson #569). Three
# distinct ways to be unable to verify — no secret, no header, a mode this build
# cannot compute — and all three must reach the same verdict.
eq "B4 every way of being unable to verify returns the same closed verdict" \
   "$($G -cE 'return SignatureVerdict::Unverifiable' "$GW")" "2"
eq "B4b the mode this build cannot compute is one of them" \
   "$($G -cE '_ => return SignatureVerdict::Unverifiable' "$GW")" "1"

# B5: blank is absent on the READ side too. This is the arm that kills the empty
# key: without it a stored blank secret still reaches the HMAC.
eq "B5 the grader treats a blank stored secret as absent" \
   "$($G -cE 'let \(Some\(secret\), Some\(header_name\)\) = \(secret, header_name\) else' "$GW")" "1"

# B6: the handler grades through the shared function rather than inline, so the
# unit tests below it are testing what the handler runs.
eq "B6 the handler grades through the function the unit tests exercise" \
   "$($G -c 'let verdict = classify_signature(' "$GW")" "1"

echo "── C. A rejected delivery is RECORDED — the order is the whole fix ──────"

# The defect was positional, not textual: every line involved was correct on its
# own and the rejection simply stood above the INSERT. So this arm compares
# positions. Both anchors are asserted first, or the comparison is vacuous.
INS_LINE=$($G -n 'INSERT INTO webhook_deliveries' "$GW" | head -1 | cut -d: -f1)
REJ_LINE=$($G -n 'if let Some(reason) = verdict.rejection()' "$GW" | head -1 | cut -d: -f1)
if [ -n "$INS_LINE" ] && [ -n "$REJ_LINE" ]; then
  ok "C0 both anchors found — delivery INSERT at :$INS_LINE, rejection at :$REJ_LINE"
  if [ "$INS_LINE" -lt "$REJ_LINE" ]; then
    ok "C1 the delivery is recorded before the request is rejected"
  else
    bad "C1 the delivery is recorded before the request is rejected" \
        "INSERT at :$INS_LINE is below the rejection at :$REJ_LINE — signature_valid can never be written FALSE"
  fi
else
  bad "C0 both anchors found" "INSERT='$INS_LINE' rejection='$REJ_LINE' — C1 would compare nothing"
fi

# C2: the value written is the verdict's own, so a rejection cannot be recorded
# as an attestation.
eq "C2 the recorded verification state comes from the verdict" \
   "$($G -c '.bind(verdict.recorded())' "$GW")" "1"

# C3: the counter and the list answer for the same population — the count sits
# above the rejection too, so a rejected delivery is counted as received.
CNT_LINE=$($G -n 'total_received = total_received + 1' "$GW" | head -1 | cut -d: -f1)
if [ -n "$CNT_LINE" ] && [ -n "$REJ_LINE" ] && [ "$CNT_LINE" -lt "$REJ_LINE" ]; then
  ok "C3 the Received counter covers every recorded delivery"
else
  bad "C3 the Received counter covers every recorded delivery" \
      "counter at ':$CNT_LINE' vs rejection at ':$REJ_LINE' — the column and the list would disagree"
fi

# C4: nothing is forwarded on a rejection. The forward loop must stay below the
# rejection, or the fix records the lie instead of stopping it.
# The same forward call appears in the replay handler ABOVE this one, so the
# anchor is the first occurrence BELOW the delivery INSERT — taking the file's
# first would measure a different function entirely.
FWD_LINE=$($G -n 'forward_to_route(&db_clone' "$GW" | awk -F: -v i="${INS_LINE:-0}" '$1 > i { print $1; exit }')
if [ -n "$FWD_LINE" ] && [ -n "$REJ_LINE" ] && [ "$REJ_LINE" -lt "$FWD_LINE" ]; then
  ok "C4 an unverified delivery is rejected before any route is loaded"
else
  bad "C4 an unverified delivery is rejected before any route is loaded" \
      "rejection at ':$REJ_LINE' vs route load at ':$FWD_LINE'"
fi

echo "── D. The shared secret does not travel to the browser ─────────────────"

# D1: window the check on the declaration itself. A bare count of the attribute
# would pass if it were attached to any other field (lesson #172).
eq "D1 the stored secret is suppressed on the serialized struct" \
   "$($G -A1 'serde(skip_serializing)' "$GW" | $G -c 'pub verify_secret: Option<String>')" "1"

# D2: ABSENCE arm with a positive control — the SPA must not declare the secret,
# because a field it declares is a field it expects to receive.
UI_SECRET=$($G -cE '^\s*(//)?\s*verify_secret:' "$UI")
if [ "$UI_SECRET" = "0" ]; then
  ok "D2 the endpoint type in the SPA does not carry the secret"
else
  bad "D2 the endpoint type in the SPA does not carry the secret" "found $UI_SECRET"
fi
eq "D2-control the same shape DOES match the form's own state field" \
   "$($G -cE 'verify_secret: ""' "$UI")" "2"

# D3: the presence answer replaces it on both ends, or the screen has no way to
# tell a working endpoint from a broken one.
eq "D3 the backend answers the presence question instead" \
   "$($G -c 'pub verify_secret_set: bool' "$GW")" "1"
eq "D3b it is derived from the stored value, not from the request" \
   "$($G -c 'fn derive_verify_secret_set(&mut self)' "$GW")" "1"
eq "D3c and it is derived on every path that returns an endpoint" \
   "$($G -c 'derive_verify_secret_set()' "$GW")" "2"
eq "D3d the SPA declares it" \
   "$($G -c 'verify_secret_set: boolean' "$UI")" "1"

echo "── E. Both states are reachable on the screen ──────────────────────────"

# E1: the form cannot create the broken state. A backend-only refusal would be a
# 400 the operator meets after filling the form in.
eq "E1 the create button is disabled while a half is missing" \
   "$($G -c 'disabled={verifyHalvesMissing}' "$UI")" "1"
eq "E1b and the condition requires both halves" \
   "$($G -cE '!epForm\.verify_secret\.trim\(\) \|\| !epForm\.verify_header\.trim\(\)' "$UI")" "1"

# E2: an endpoint already stored in the broken state has no repair path except
# delete-and-recreate, so the list has to say so — a fix nobody can see is a
# severed control (lesson #577).
eq "E2 the list marks an endpoint that can verify nothing" \
   "$($G -cE 'ep\.verify_mode !== "none" && !ep\.verify_secret_set' "$UI")" "1"

# E3: the badge the FALSE state finally makes reachable is still rendered.
eq "E3 the delivery list still renders the rejected state" \
   "$($G -c 'd.signature_valid === null ?' "$UI")" "1"

echo "── F. The guide describes what the code does ───────────────────────────"

# F1: ABSENCE arm — the guide called the secret optional while the mode required
# it, which is the sentence an operator followed into the broken state.
OPTIONAL=$($G -c 'Optional shared secret' "$GUIDE")
if [ "$OPTIONAL" = "0" ]; then
  ok "F1 the guide no longer calls the secret optional under a chosen algorithm"
else
  bad "F1 the guide no longer calls the secret optional under a chosen algorithm" "found $OPTIONAL"
fi
eq "F1-control the guide still documents the secret at all" \
   "$($G -c 'HMAC Secret' "$GUIDE")" "1"

eq "F2 the guide documents the rejecting state and its repair" \
   "$($G -c 'no secret — rejecting' "$GUIDE")" "1"
eq "F3 the guide states the secret is never returned" \
   "$($G -c 'never sent back to the browser' "$GUIDE")" "1"

echo "── G. The public door is still the only unauthenticated one ────────────"

# G1: the inbound gateway is public by design — a webhook carries a secret, not a
# session. Pinned so a future refactor cannot quietly authenticate it (which
# would break every sender) or add a second unauthenticated door beside it.
eq "G1 the inbound gateway route is registered exactly once" \
   "$($G -c 'api/webhooks/gateway/{token}' "$ROUTES")" "1"
# G2: the population is DERIVED from the file, not listed here — a list is
# satisfied by the handler it forgot (lesson #551). Every handler in the module
# is admin-gated except the one public inbound door pinned by G1.
HANDLERS=$($G -c '^pub async fn ' "$GW")
[ "$HANDLERS" -ge 8 ] && ok "G2-control the module still declares $HANDLERS handlers" \
  || bad "G2-control" "only $HANDLERS handlers found — G2 would compare nothing"
eq "G2 every handler except the public inbound door is admin-gated" \
   "$($G -c 'AdminUser(claims): AdminUser' "$GW")" "$((HANDLERS - 1))"

echo "── H. The controls exist, and a replay is the same delivery ────────────"

# Two severed toggles and a divergent replay, all in this one module.
#
# `webhook_endpoints.enabled` gates the public inbound door and
# `webhook_routes.enabled` gates every outbound forward — and nothing in the
# product could write either. Both columns were born TRUE and stayed TRUE for the
# life of the row, so shutting a public door meant DELETING the endpoint, which
# cascades away every delivery and every route recorded against it. The only way
# to stop traffic was to destroy the record of the traffic that had arrived.
#
# And replay forwarded to every enabled route regardless of its filter, while the
# inbound path skipped the ones that did not match — so replaying sent the payload
# to destinations the operator had deliberately excluded.

# H0: the two handler declarations anchor the positional arm below. They are the
# anchors precisely because no plant against the filter or the toggles touches
# them — an anchor that the defect can delete makes its dependent arms go MISSING
# rather than RED, which reads as caught and is not.
REPLAY_FN=$($G -n '^pub async fn replay_delivery' "$GW" | head -1 | cut -d: -f1)
RECV_FN=$($G -n '^pub async fn receive_webhook' "$GW" | head -1 | cut -d: -f1)
if [ -n "$REPLAY_FN" ] && [ -n "$RECV_FN" ] && [ "$REPLAY_FN" -lt "$RECV_FN" ]; then
  ok "H0 both forwarding handlers found — replay at :$REPLAY_FN, inbound at :$RECV_FN"
else
  bad "H0 both forwarding handlers found" "replay='$REPLAY_FN' inbound='$RECV_FN' — H1 would compare nothing"
fi

# H1: the filter decision is taken on BOTH paths. Positional, not a count: one
# call site inside replay and one below the inbound handler. A count of two is
# satisfied by two calls in the same function.
# Keyed on the CALL shape `route_admits(&`, never the bare name: the helper's own
# DEFINITION sits in the Helpers section below both handlers, so an arm counting
# the bare name after the inbound handler is satisfied by the definition alone and
# can never fail for that path. It did not, until a plant that severed the inbound
# filter came back green.
eq "H0b the filter decision has exactly one definition" \
   "$($G -c '^fn route_admits(' "$GW")" "1"
if [ -n "$REPLAY_FN" ] && [ -n "$RECV_FN" ]; then
  IN_REPLAY=$($G -n 'route_admits(&' "$GW" | awk -F: -v a="$REPLAY_FN" -v b="$RECV_FN" '$1>a && $1<b' | wc -l)
  IN_RECV=$($G -n 'route_admits(&' "$GW" | awk -F: -v b="$RECV_FN" '$1>b' | wc -l)
  if [ "$IN_REPLAY" -ge 1 ] && [ "$IN_RECV" -ge 1 ]; then
    ok "H1 replay and the inbound door take the same routing decision"
  else
    bad "H1 replay and the inbound door take the same routing decision" \
        "filter calls: replay=$IN_REPLAY inbound=$IN_RECV — a replay would reach routes the delivery never did"
  fi
fi

# H2: the two gates have writers, one per table. Keyed on the table so a second
# writer of one column cannot stand in for the missing writer of the other.
eq "H2a the inbound door can be closed without deleting its history" \
   "$($G -c 'UPDATE webhook_endpoints SET enabled' "$GW")" "1"
eq "H2b forwarding to one destination can be stopped without deleting the route" \
   "$($G -c 'UPDATE webhook_routes r SET enabled' "$GW")" "1"

# H3: and the operator can see and reach both. The columns were declared on the
# SPA's types and read by nothing at all, which is how they stayed severed while
# the screen looked complete.
eq "H3 the endpoint's state is rendered and toggleable" \
   "$($G -c 'toggleEndpoint(ep)' "$UI")" "1"
eq "H3b the route's state is rendered and toggleable" \
   "$($G -c 'toggleRoute(r)' "$UI")" "1"

# H4: replaying a delivery the gateway REFUSED stays possible — the guide
# documents it and the usual cause is a sender misconfigured at the far end —
# but it is no longer one click beside a red Invalid badge.
eq "H4 replaying a refused delivery asks first" \
   "$($G -c 'd.signature_valid === false &&' "$UI")" "1"

# H5: and it leaves a trace. Replay sends an externally-supplied body onward
# under the panel's own credentials and was the one forwarding action here that
# recorded nothing.
eq "H5 a replay is written to the activity log" \
   "$($G -c 'webhook_delivery.replay' "$GW")" "1"

echo
echo "PASS $PASS / FAIL $FAIL"
[ "$FAIL" -eq 0 ]
