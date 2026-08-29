#!/usr/bin/env bash
# billing-webhook-plan-source-pin-e2e.sh — Stripe billing correctness, found by
# s424's audit-coverage fan-out on billing.rs (never independently audited
# before this session; only prior mention was an incidental s287 aside).
#
#   1. customer.subscription.updated read the plan tier from `metadata.plan`,
#      a snapshot frozen at ORIGINAL checkout time. A Customer Portal
#      upgrade/downgrade changes the subscription's price but never touches
#      metadata, so the recorded plan (and its server_limit) permanently
#      desynced from what Stripe was actually billing — in either direction —
#      with no reconciliation. Fixed: resolve the plan from the subscription's
#      CURRENT price via a reverse `stripe_price_{plan}` settings lookup,
#      falling back to metadata only if no configured price matches.
#   2. create_checkout never checked for an existing active subscription
#      before starting a new Stripe Checkout session — an upgrade click (or a
#      double-click) could spawn a second subscription and desync
#      stripe_subscription_id. Fixed: reject with 409 CONFLICT when the user
#      already has one, pointing them at the Customer Portal instead.
#   3. Two of the three Stripe API response handlers in this file (customer
#      creation, customer_portal) never checked for Stripe's own `error` key
#      before reading a success field — a 4xx from Stripe degraded to a
#      confusing generic 502 ("missing customer id" / "missing portal URL")
#      instead of surfacing the actual cause. Fixed: both sites now check
#      `error` first, matching the pattern the third call site (checkout
#      session creation) already used.
#
# Pure source analysis: no box, no network, no build, no live Stripe calls.
#
# NO PIPES INTO `grep -q` — under `set -o pipefail` grep -q closes the pipe on
# its first match and the arm goes red on correct code. Every arm uses a
# here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

BILLING=panel/backend/src/routes/billing.rs
[ -f "$BILLING" ] || bad "MISSING SUBJECT FILE: $BILLING"

echo "== §A  plan_from_price_id resolves the CURRENT price, not frozen metadata =="

HELPER=$(awk '/^async fn plan_from_price_id\(/{i=1} i{print} i && /^}$/{exit}' "$BILLING")
NHELP=$(grep -c . <<< "$HELPER")
if [ "$NHELP" -ge 8 ]; then
  ok "A1-control plan_from_price_id body extracted — $NHELP lines"
else
  bad "A1-control plan_from_price_id body extracted — only $NHELP lines (the extractor broke)"
fi

# A2: an empty price_id must short-circuit to None — never issue a query that
# could accidentally match a NULL/empty settings value.
if grep -qE 'if price_id\.is_empty\(\)' <<< "$HELPER" && grep -qE 'return None;' <<< "$HELPER"; then
  ok "A2 an empty price_id returns None before querying"
else
  bad "A2 must guard on an empty price_id — otherwise an empty string could spuriously match an unset setting"
fi

# A3: resolves against all three configured tiers, not a subset.
if grep -qE "stripe_price_starter', 'stripe_price_pro', 'stripe_price_agency" <<< "$HELPER"; then
  ok "A3 checks all three configured price settings (starter/pro/agency)"
else
  bad "A3 must check all three plan tiers — omitting one would silently fall back to metadata for that tier"
fi

# A4: match is by VALUE (the price ID), returning the plan KEY with its
# 'stripe_price_' prefix stripped.
if grep -qE '\.find\(\|\(_, v\)\| v == price_id\)' <<< "$HELPER" \
   && grep -qE 'trim_start_matches\("stripe_price_"\)' <<< "$HELPER"; then
  ok "A4 matches by price value and returns the bare plan key"
else
  bad "A4 must match on price value and strip the settings-key prefix to yield a plan name plan_def() understands"
fi

echo "== §B  customer.subscription.updated calls the resolver instead of trusting metadata =="

UPDATED=$(awk '/"customer\.subscription\.updated" =>/{i=1} i{print} i && /^        }$/{exit}' "$BILLING")
NUPD=$(grep -c . <<< "$UPDATED")
if [ "$NUPD" -ge 15 ]; then
  ok "B1-control customer.subscription.updated arm extracted — $NUPD lines"
else
  bad "B1-control customer.subscription.updated arm extracted — only $NUPD lines (the extractor broke)"
fi

# B2: the price is read from the subscription's item list, not a top-level field.
if grep -qE '\["items"\]\["data"\]\[0\]\["price"\]\["id"\]' <<< "$UPDATED"; then
  ok "B2 reads the subscription's current line-item price"
else
  bad "B2 must read items.data[0].price.id — that's the only field Stripe actually updates on a plan change"
fi

# B3: plan_from_price_id is actually called and its result used, with a
# metadata fallback ONLY in the None arm (not as the primary source).
if grep -qE 'plan_from_price_id\(&state\.db, price_id\)\.await' <<< "$UPDATED" \
   && grep -qE 'None => sub\["metadata"\]\["plan"\]' <<< "$UPDATED"; then
  ok "B3 resolves via plan_from_price_id, falling back to metadata only on no match"
else
  bad "B3 must call plan_from_price_id and use metadata only as the None-arm fallback — trusting metadata directly reintroduces the desync bug"
fi

# B4 POSITIONAL: the price_id read must precede the plan_def() call that
# determines the persisted server_limit — resolving it after would be dead code.
PRICE_LINE=$(grep -n '\["items"\]\["data"\]\[0\]\["price"\]\["id"\]' <<< "$UPDATED" | head -1 | cut -d: -f1)
PLANDEF_LINE=$(grep -n 'plan_def(plan\.as_str())' <<< "$UPDATED" | head -1 | cut -d: -f1)
if [ -n "$PRICE_LINE" ] && [ -n "$PLANDEF_LINE" ] && [ "$PRICE_LINE" -lt "$PLANDEF_LINE" ]; then
  ok "B4 the price lookup (line $PRICE_LINE) precedes plan_def() (line $PLANDEF_LINE)"
else
  bad "B4 the price lookup must precede plan_def — price=${PRICE_LINE:-none} plandef=${PLANDEF_LINE:-none}"
fi

echo "== §C  create_checkout refuses a second active subscription =="

CHECKOUT=$(awk '/^pub async fn create_checkout\(/{i=1} i{print} i && /^}$/{exit}' "$BILLING")
NCHK=$(grep -c . <<< "$CHECKOUT")
if [ "$NCHK" -ge 60 ]; then
  ok "C1-control create_checkout body extracted — $NCHK lines"
else
  bad "C1-control create_checkout body extracted — only $NCHK lines (the extractor broke)"
fi

# C2: the initial user lookup now also selects stripe_subscription_id.
if grep -qE 'SELECT stripe_customer_id, email, stripe_subscription_id FROM users' <<< "$CHECKOUT"; then
  ok "C2 the user lookup selects stripe_subscription_id"
else
  bad "C2 must select stripe_subscription_id — otherwise there is nothing to guard against"
fi

# C3: an existing subscription is rejected with 409 CONFLICT, not silently allowed.
if grep -qE 'user\.2\.is_some\(\)' <<< "$CHECKOUT" && grep -qE 'StatusCode::CONFLICT' <<< "$CHECKOUT"; then
  ok "C3 an existing subscription is rejected with 409 CONFLICT"
else
  bad "C3 must reject when stripe_subscription_id is already set — otherwise a second checkout can spawn a duplicate subscription"
fi

# C4 POSITIONAL: the guard must run BEFORE any Stripe API call (customer
# creation or checkout session creation) — checking after would have already
# spent a real Stripe round-trip (and possibly created a duplicate customer).
GUARD_LINE=$(grep -n 'user\.2\.is_some\(\)' <<< "$CHECKOUT" | head -1 | cut -d: -f1)
STRIPE_CALL_LINE=$(grep -n 'post("https://api.stripe.com' <<< "$CHECKOUT" | head -1 | cut -d: -f1)
if [ -n "$GUARD_LINE" ] && [ -n "$STRIPE_CALL_LINE" ] && [ "$GUARD_LINE" -lt "$STRIPE_CALL_LINE" ]; then
  ok "C4 the subscription guard (line $GUARD_LINE) runs before any Stripe API call (line $STRIPE_CALL_LINE)"
else
  bad "C4 the guard must precede every Stripe call — guard=${GUARD_LINE:-none} first_call=${STRIPE_CALL_LINE:-none}"
fi

echo "== §D  every Stripe response site checks the error key before reading a success field =="

# D1: customer-creation path (previously missing).
CUSTOMER_CREATE=$(awk '/Create Stripe customer/{i=1} i{print} i && /^        cid$/{exit}' <<< "$CHECKOUT")
if grep -qE 'body\.get\("error"\)' <<< "$CUSTOMER_CREATE"; then
  ok "D1 customer-creation now checks Stripe's error key"
else
  bad "D1 customer creation must check body.get(\"error\") before reading body[\"id\"] — a 4xx degrades to a confusing generic 502 otherwise"
fi

# D2: checkout-session creation (positive control — this one already had the check).
if grep -qE 'session\.get\("error"\)' <<< "$CHECKOUT"; then
  ok "D2-control checkout-session creation still checks the error key (pre-existing)"
else
  bad "D2-control checkout-session creation lost its pre-existing error-key check"
fi

# D3: customer_portal (previously missing).
PORTAL=$(awk '/^pub async fn customer_portal\(/{i=1} i{print} i && /^}$/{exit}' "$BILLING")
if grep -qE 'session\.get\("error"\)' <<< "$PORTAL"; then
  ok "D3 customer_portal now checks Stripe's error key"
else
  bad "D3 customer_portal must check session.get(\"error\") before reading session[\"url\"] — a 4xx (e.g. deleted customer) degrades to a confusing generic 502 otherwise"
fi

echo
printf 'billing-webhook-plan-source: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
