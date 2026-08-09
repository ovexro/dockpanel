#!/usr/bin/env bash
# Regression pins for s334 — a feature the manifest advertises must have a
# reachable state, and the address the panel prints must be the address the
# handler reads.
#
#   W1  THE INTEGRATION WAS NEVER CONFIGURABLE, ON ANY INSTALL, SINCE IT SHIPPED.
#       `update_config` is the only writer of `whmcs_config` and it is spelled
#       `INSERT ... ON CONFLICT ((true)) DO UPDATE`. Postgres resolves an
#       ON CONFLICT arbiter at PARSE-ANALYSIS time against a real unique or
#       exclusion constraint, and the table's only index was the primary key on a
#       `gen_random_uuid()` default. So the statement raised before it examined a
#       row — on the FIRST save as well as re-saves — and the table could never
#       hold anything. Every reader downstream was therefore permanently dead:
#       `get_config` answered not-configured and the webhook answered 404. Proven
#       by EXPLAIN with two controls (a valid arbiter plans; the same target on a
#       table that HAS a constraint fails identically), so the defect is the
#       target, not the table. Origin `3feb92f`, 2026-03-28.
#
#   W2  THE SECRET WAS RETURNED BUT NOT STORED. The old statement bound a freshly
#       minted webhook secret in its VALUES list and omitted the column from its
#       DO UPDATE SET, then returned the fresh value to the operator regardless.
#       Once W1 was fixed, every re-save would have handed back a secret the
#       database did not hold.
#
#   W3  THE SECRET WAS REGENERATED ON EVERY SAVE. Minting was unconditional, so
#       saving the screen would silently invalidate the hook the operator had
#       already wired into WHMCS. Tracked before this session but never reachable
#       to observe, because of W1.
#
#   W4  THE FLAG READ FELL BACK TO DEFAULTS ON A DATABASE ERROR. `.ok().flatten()`
#       turned "cannot read the current config" into "there is no config", which
#       re-enables billing-driven suspension for an operator who turned it off —
#       the exact clobber the surrounding comment exists to prevent.
#
#   W5  THE PRINTED WEBHOOK URL AUTHENTICATED NOTHING. The panel renders the hook
#       address with the secret as a query parameter; the handler took only a JSON
#       body and read the secret from it, so the advertised URL answered 401.
#
# POLARITY, WRITTEN DOWN BEFORE THE RUN (#335 — a false green hides inside a batch
# of reds): W1-W5 are RED at v2.91.0 and green here. W6 and W7 are CONTROLS and are
# green at BOTH tags on purpose — they assert the fix did not weaken the webhook's
# authentication while making it reachable.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
# Boolean grep that CONSUMES ALL INPUT.
#
# `producer | grep -q PAT` is a race under `set -o pipefail`: grep -q exits at
# the first match, the producer dies of SIGPIPE, and the pipeline reports 141 —
# so a SUCCESSFUL match is read as a failure. Measured here at ~2 in 400 runs on
# a 2,250-line subject, which is why it surfaced as an unreproducible red rather
# than a broken arm. `grep -c` reads to EOF, and its exit status is still 0 on a
# match, so this is the same boolean with no early exit.
qgrep() { grep -c "$@" >/dev/null; }
BE=panel/backend/src
FE=panel/frontend/src
MIG=panel/backend/migrations

for d in "$BE" "$FE" "$MIG"; do
  [ -d "$d" ] || { echo "missing dir: $d — refusing to report a clean sweep"; exit 1; }
done

# Strip comments before asking a question about CODE, so an arm cannot be
# satisfied by the prose that narrates the check (#149). Copy of the FIXED
# stripper — a block comment is recognised only where one is written (#136).
code() {
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

WH="$BE/routes/whmcs.rs"
INT="$FE/pages/Integrations.tsx"
[ -f "$WH" ]  || { echo "missing $WH — refusing to report a clean sweep";  exit 1; }
[ -f "$INT" ] || { echo "missing $INT — refusing to report a clean sweep"; exit 1; }

echo ""
echo "── whmcs config writable pins ──"

# W1. The ON CONFLICT target must be backed by a real constraint somewhere in the
# migrations. Keyed on the CREATE, not on the table name alone — the table name
# appears in its own creating migration, which establishes nothing.
#
# The enumeration is asserted first: if the glob finds no migrations at all, an
# absence arm would pass having examined nothing (#143).
MIG_COUNT=$(find "$MIG" -name '*.sql' | wc -l)
if [ "$MIG_COUNT" -lt 10 ]; then
  bad "W1 only $MIG_COUNT migrations found — the subject list is implausible, refusing to judge"
elif [ "$(grep -rlE 'CREATE +UNIQUE +INDEX[^;]*whmcs_config' "$MIG" | wc -l)" -gt 0 ]; then
  ok "W1 a unique constraint backs the singleton upsert, so the config table can hold a row"
else
  bad "W1 nothing in $MIG_COUNT migrations creates a unique constraint on whmcs_config — the only writer cannot execute"
fi

# W2. The upsert's DO UPDATE SET must assign the webhook secret column. Scoped to
# the statement, bounded on its closing quote, so a mention elsewhere in the file
# cannot satisfy it.
UPSERT=$(code "$WH" | perl -0777 -ne 'print $1 if /(INSERT INTO whmcs_config.*?DO UPDATE SET.*?)"\s*\)/s')
if [ -z "$UPSERT" ]; then
  bad "W2 could not extract the whmcs_config upsert — the arm has no subject, not a passing result"
elif printf '%s' "$UPSERT" | qgrep -E 'webhook_secret *='; then
  ok "W2 the upsert stores the webhook secret it returns"
else
  bad "W2 the upsert omits webhook_secret from DO UPDATE SET, so a re-save returns a secret it did not store"
fi

# W3. Minting must be conditional on there being no stored secret. Keyed on the
# operation (a fallback), not on the mint call, which exists in both versions.
if code "$WH" | qgrep -E 'unwrap_or_else\(\|\| *uuid::Uuid::new_v4'; then
  ok "W3 a webhook secret is minted only when none is stored, so a save cannot invalidate the configured hook"
else
  bad "W3 the webhook secret is minted unconditionally — every save rotates it out from under WHMCS"
fi

# W4. The current-config read must propagate rather than substitute defaults.
CUR=$(code "$WH" | perl -0777 -ne 'print $1 if /(SELECT auto_provision.*?;)/s')
if [ -z "$CUR" ]; then
  bad "W4 could not extract the current-config read — the arm has no subject"
elif printf '%s' "$CUR" | qgrep 'ok()'; then
  bad "W4 the current-config read still swallows a database error, so a blip resets the operator's flags"
else
  ok "W4 a failed current-config read is an error, not a silent reset to defaults"
fi

# W5. The webhook must accept the secret from the query string, because that is
# the shape the panel prints. Two halves: the handler takes a Query extractor, and
# the verification actually consults it.
if code "$WH" | qgrep -E 'Query\([a-z_]+\): *Query<' \
   && code "$WH" | qgrep -E '\.or\([a-z_]+\.secret\.as_deref\(\)\)'; then
  ok "W5 the webhook reads the secret from the query string as well as the body, so the printed URL works"
else
  bad "W5 the webhook still ignores a query-string secret, so the address the panel prints answers 401"
fi

# W6. CONTROL — green at both tags. The comparison must stay constant-time; a fix
# that made the hook reachable while weakening its authentication would be worse
# than the defect.
if code "$WH" | qgrep 'ct_eq'; then
  ok "W6 CONTROL the webhook secret is still compared in constant time"
else
  bad "W6 CONTROL the constant-time comparison is gone — the fix weakened authentication"
fi

# W7. CONTROL — green at both tags. An unconfigured panel must still refuse hooks
# rather than opening a window while the secret is absent.
if code "$WH" | qgrep -E 'Webhook secret not configured'; then
  ok "W7 CONTROL a webhook still fails closed when no secret is configured"
else
  bad "W7 CONTROL the unconfigured-webhook refusal is gone — the endpoint may be open"
fi

# W8. The panel still prints the hook address, and it still carries the secret —
# otherwise W5 would be satisfied by an address nobody is given.
if qgrep -E 'whmcs/webhook\?secret=' "$INT"; then
  ok "W8 the panel still hands the operator a hook URL carrying the secret"
else
  bad "W8 the printed hook URL no longer carries the secret — W5 now pins an address nobody is shown"
fi

echo ""
echo "── whmcs config writable pins: PASS $PASS / FAIL $FAIL ──"
[ "$FAIL" -eq 0 ] || exit 1
