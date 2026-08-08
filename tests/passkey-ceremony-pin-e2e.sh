#!/usr/bin/env bash
# passkey-ceremony-pin-e2e.sh — s328
#
# ONE PROPERTY: the WebAuthn ceremony's own checks are arranged so that each one
# means what it says.
#
# Until v2.86.0 `passkeys.rs` had NO tests of any kind — no `cfg(test)` module,
# and not one assertion in any of the 46 pin suites touching origin, rpIdHash,
# user-presence, the signature counter or the signature itself. Five live checks,
# zero coverage. This suite pins the properties that are facts about ARRANGEMENT;
# the ones that are facts about a return value are unit tests in the file itself,
# because a truth table that executes cannot be vacuous the way a grep can.
#
#   §A  NEITHER DOOR INDEXES PAST ITS OWN LENGTH GUARD.
#       ← the headline defect this release fixes
#       `authData` is rpIdHash(32) ‖ flags(1) ‖ signCount(4). `register_complete`
#       bounded it at 32 and then read index [32], so a 32-byte buffer passed the
#       guard and panicked the handler — and the payload is attacker-chosen,
#       because it only has to equal SHA256(rp_id), which is public. The twin in
#       `auth_complete` always used 37. The ASYMMETRY between the two doors was
#       the bug, which is why §A is a membership test over both and not a check
#       of one.
#
#   §B  THE CLONE CHECK RUNS AFTER THE SIGNATURE.
#       WebAuthn L2 §7.2 orders it that way and the reason is operational. Above
#       the verify, the branch fires on forged `authData` with a garbage
#       signature — `auth_begin` hands out challenges unauthenticated, rpIdHash
#       is public and the UP bit is a constant — so the one refusal in this file
#       that says "clone" was reachable by a stranger AND returned without
#       recording an attempt, unlike both its siblings. That is also why the
#       detector stayed mute: an alert wired to a branch an outsider can trigger
#       is a fabricated-alert writer, so the reorder had to land first and the
#       audit row second. B4 pins that it is `audit_log` and NOT
#       `record_suspicious_event`, which auto-activates system-wide lockdown at
#       five events in ten minutes — routing a credential failure through it
#       would let a cloned key black the panel out for every non-admin.
#
#   §C  THE COUNTER RULE IS A NAMED FUNCTION, AND IT IS THE SPEC'S RULE.
#       The spec applies the comparison when EITHER side is non-zero. This panel
#       tested both with `&&` from `adce010` until v2.86.0, exempting the one
#       shape that matters: a stored counter above zero presented with ZERO —
#       the cheapest forgery there is, and one whose acceptance writes the zero
#       back. Pinned as a free function so the truth table can be executed.
#
#   §D  BOTH COMPLETE-DOORS CHECK WHICH CEREMONY THE CHALLENGE CAME FROM.
#       `register_complete` has since the feature landed; `auth_complete` bound
#       it to `_challenge_data` and discarded it. No attack is known — this door
#       derives the account from the credential row, never from the challenge's
#       binding — so D is a symmetry pin, not a vulnerability pin.
#
#   §E  THE COUNTER WRITE CANNOT WALK BACKWARDS.
#
#   §F  THE FILE HAS EXECUTABLE COVERAGE AT ALL.
#
#   §G  CONTEXT — arms green at BOTH tags, so a harness measuring nothing cannot
#       read as a pass. Each owes its own mutation.
#
# ⚠ NOT SHIPPED, DELIBERATELY, and G4 pins the non-ship: a user-verification
# requirement. The UV bit (0x04) is still never tested, so a passkey remains
# possession-only while passkey login deliberately skips 2FA. It is NOT fixed
# here because UV is a property of the CEREMONY, not of the credential — nothing
# recorded at registration binds a later assertion — so enforcing it at the
# enrolment door protects nothing, and enforcing it at the login door is
# RETROACTIVE: every already-enrolled credential on a PIN-less roaming key stops
# working, on every install, with no column to grandfather from. That needs a
# `uv` column populated at registration and a virtual-authenticator harness to
# prove the flip, which is a ship of its own.
#
# RUN AGAINST v2.85.0: the §G arms are green by design and each was killed by its
# own mutation. Everything else is RED there.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  Passkey ceremony — source pins (s328)"
echo "=============================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# Strip comments before matching. This ship writes long comments into the very
# file it pins — they name `record_suspicious_event`, `< 37` and the predicate
# itself — so an arm over RAW source would be satisfied by its own prose, and an
# ABSENCE arm would be falsely RED. ⚠ Copied from the FIXED stripper (s294): a
# block comment is recognised only where one is written, or a `/*` inside a
# string literal eats the file.
code() {
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
    s{\{/\*.*?\*/\}}{}gs;
  ' "$1"
}

has()  { grep -qE -- "$2" <<< "$1"; }
hasf() { grep -qF -- "$2" <<< "$1"; }
cnt()  { grep -cF -- "$2" <<< "$1"; }

# A function body, bounded on ITS OWN braces, anchored on `fn NAME(` so a rename
# to a longer name cannot survive on the prefix (s325 #299). ⚠ An extraction that
# comes back EMPTY must be an ERROR, never an absence (s327 #308): an absence arm
# over an empty string is green on every possible tree.
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

PK="panel/backend/src/routes/passkeys.rs"
PK_C=$(code "$PK")

if [ -z "$PK_C" ]; then
  bad "FATAL could not read or strip $PK — every arm below would be vacuous"
  echo; printf '  PASS %d   FAIL %d\n' "$PASS" "$FAIL"; exit 1
fi

REG_BODY=$(fnbody "$PK_C" "register_complete")
AUTH_BODY=$(fnbody "$PK_C" "auth_complete")

# The enumeration guard for everything below (s295 #143). An empty body makes
# every absence arm over it green on any tree whatsoever.
if [ -n "$REG_BODY" ] && [ -n "$AUTH_BODY" ]; then
  ok "A0 both ceremony handlers extracted (register_complete $(wc -l <<< "$REG_BODY")L, auth_complete $(wc -l <<< "$AUTH_BODY")L)"
else
  bad "A0 a handler body extracted EMPTY — every arm below is vacuous; fix fnbody before reading any result in this file"
  echo; printf '  PASS %d   FAIL %d\n' "$PASS" "$FAIL"; exit 1
fi

echo
echo "── §A neither door indexes past its own length guard ──"

# A1 — MEMBERSHIP, not a count. Discover every handler that reads the flags byte
# at a fixed offset, then assert each one bounds the buffer at the full 37-byte
# header first. Written this way because the defect was ONE of two doors being
# wrong: a count of correct guards would have been satisfied by the good twin.
INDEXERS=""
for f in register_complete auth_complete; do
  b=$(fnbody "$PK_C" "$f")
  hasf "$b" 'auth_data_bytes[32]' && INDEXERS="$INDEXERS $f"
done
INDEXERS=$(echo $INDEXERS)
N_IDX=$(wc -w <<< "$INDEXERS")

if [ "$N_IDX" -ge 2 ]; then
  ok "A1 discovered $N_IDX handlers that index the flags byte:$INDEXERS"
else
  bad "A1 discovered only $N_IDX flags-byte reader(s) — the pattern broke and A2 is vacuous"
fi

UNGUARDED=""
for f in $INDEXERS; do
  b=$(fnbody "$PK_C" "$f")
  hasf "$b" 'auth_data_bytes.len() < 37' || UNGUARDED="$UNGUARDED $f"
done
if [ "$N_IDX" -ge 2 ] && [ -z "$UNGUARDED" ]; then
  ok "A2 every handler that reads auth_data_bytes[32] bounds the buffer at 37 first"
else
  bad "A2 handler(s) index the flags byte behind a short guard:${UNGUARDED:- none-discovered} — a 32-byte authData panics the task"
fi

# A3 — the absence, stated directly. The old bound must be gone from the file.
if ! hasf "$PK_C" 'auth_data_bytes.len() < 32'; then
  ok "A3 no door bounds authData at 32 — the header is 32+1+4 and the guard must cover all of it"
else
  bad "A3 a door still bounds authData at 32 then reads index [32] — that is the panic"
fi

echo
echo "── §B the clone check runs after the signature ──"

VERIFY_LN=$(grep -n 'verifying_key.verify(' <<< "$AUTH_BODY" | head -1 | cut -d: -f1)
COUNTER_LN=$(grep -n 'counter_regressed(' <<< "$AUTH_BODY" | head -1 | cut -d: -f1)

if [ -n "$VERIFY_LN" ] && [ -n "$COUNTER_LN" ]; then
  ok "B1 both landmarks found in auth_complete (verify at +${VERIFY_LN}, counter at +${COUNTER_LN})"
else
  bad "B1 could not locate verify(${VERIFY_LN:-missing}) or counter_regressed(${COUNTER_LN:-missing}) — B2 would be vacuous"
fi

if [ -n "$VERIFY_LN" ] && [ -n "$COUNTER_LN" ] && [ "$COUNTER_LN" -gt "$VERIFY_LN" ]; then
  ok "B2 the counter regression test runs AFTER signature verification, so reaching it costs the private key"
else
  bad "B2 the counter test precedes signature verification — it fires on forged authData, unthrottled, and any alert on it is writable by a stranger"
fi

# B3 — the branch must cost the caller something. Its two siblings in this
# handler both push to login_attempts; the counter branch did not, which is what
# made it an unthrottled probe.
CB=$(awk '/counter_regressed\(/{f=1} f{print} f&&/^    }$/{exit}' <<< "$AUTH_BODY")
if [ -n "$CB" ] && hasf "$CB" 'login_attempts'; then
  ok "B3 a detected clone records a login attempt, like every other credential refusal in this handler"
else
  bad "B3 the clone branch does not record an attempt — it is a free retry, and free retries are how a counter gets probed"
fi

# ⚠ Scoped to the CLONE BRANCH, never to the handler. `auth_complete` already
# called `audit_log` for the successful login, so a handler-wide arm here is
# satisfied by that unrelated call and stays green while the detector is mute —
# it read green at v2.85.0, where the clone branch has no audit row at all
# (s327 #310: key on the act, not on a token that appears elsewhere).
if [ -n "$CB" ] && hasf "$CB" 'security_hardening::audit_log' && ! hasf "$CB" 'record_suspicious_event'; then
  ok "B4 the clone branch itself writes an audit row, and not through record_suspicious_event"
else
  bad "B4 the clone detector is mute, or it routes through record_suspicious_event — which auto-locks the panel at 5 events in 10 minutes, so a failing cloned key would black out every non-admin"
fi

echo
echo "── §C the counter rule is the spec's rule, and it is testable ──"

PRED=$(fnbody "$PK_C" "counter_regressed")
if [ -n "$PRED" ]; then
  ok "C1 counter_regressed is a named function, so its truth table can be executed rather than grepped"
else
  bad "C1 counter_regressed does not exist — the rule is inlined again and only a grep can reach it"
fi

if [ -n "$PRED" ] && has "$PRED" 'stored > 0 \|\| new > 0'; then
  ok "C2 the rule fires when EITHER side is non-zero, per WebAuthn L2 §7.2"
else
  bad "C2 the rule tests both sides with && — a stored counter presented with ZERO is exempted, which is the cheapest forgery and it writes the zero back"
fi

# C3 — the call site must USE the predicate, not carry a second copy of the rule.
if ! has "$AUTH_BODY" 'new_count <= stored_count'; then
  ok "C3 auth_complete delegates to the predicate instead of re-inlining the comparison"
else
  bad "C3 auth_complete inlines the counter comparison again — two copies of one rule, and the tests only cover one"
fi

echo
echo "── §D both complete-doors check which ceremony issued the challenge ──"

UNCHECKED=""
for f in register_complete auth_complete; do
  b=$(fnbody "$PK_C" "$f")
  hasf "$b" 'ChallengeData::' || UNCHECKED="$UNCHECKED $f"
done
if [ -z "$UNCHECKED" ]; then
  ok "D1 both complete-doors inspect the challenge variant"
else
  bad "D1 door(s) accept a challenge without checking which ceremony minted it:$UNCHECKED"
fi

if hasf "$AUTH_BODY" 'ChallengeData::Authentication'; then
  ok "D2 auth_complete requires an Authentication challenge specifically"
else
  bad "D2 auth_complete no longer pins the variant — a registration challenge satisfies the login door"
fi

if ! hasf "$AUTH_BODY" '_challenge_data'; then
  ok "D3 the challenge is consumed, not bound-and-discarded"
else
  bad "D3 auth_complete binds the challenge to a discard again — the variant it carries is unread"
fi

echo
echo "── §E the stored counter cannot walk backwards ──"

if hasf "$AUTH_BODY" 'AND $1 > sign_count'; then
  ok "E1 the counter write is conditional, closing the read/write window between two assertions in flight"
else
  bad "E1 the counter write is unconditional — a lower assertion landing last rewinds the stored counter, which is the state the clone check exists to notice"
fi

echo
echo "── §F the file has executable coverage at all ──"

if hasf "$(cat "$PK")" '#[cfg(test)]'; then
  ok "F1 passkeys.rs carries a test module — it had none from adce010 until v2.86.0"
else
  bad "F1 passkeys.rs has no test module; every property in this file is grep-only again"
fi

N_TESTS=$(cnt "$(cat "$PK")" '#[test]')
if [ "$N_TESTS" -ge 12 ]; then
  ok "F2 $N_TESTS executable tests cover the ceremony's pure functions"
else
  bad "F2 only $N_TESTS executable tests — the truth table or the parser edges were dropped"
fi

echo
echo "── §G context: green at BOTH tags, each killed by its own mutation ──"

UP_MISSING=""
for f in register_complete auth_complete; do
  b=$(fnbody "$PK_C" "$f")
  hasf "$b" 'flags & 0x01' || UP_MISSING="$UP_MISSING $f"
done
if [ -z "$UP_MISSING" ]; then
  ok "G1 (context) both doors still require the user-present bit"
else
  bad "G1 door(s) stopped requiring user presence:$UP_MISSING"
fi

ORIGIN_MISSING=""
for f in register_complete auth_complete; do
  b=$(fnbody "$PK_C" "$f")
  hasf "$b" 'Origin mismatch' || ORIGIN_MISSING="$ORIGIN_MISSING $f"
done
if [ -z "$ORIGIN_MISSING" ]; then
  ok "G2 (context) both doors still pin the origin"
else
  bad "G2 door(s) stopped pinning the origin:$ORIGIN_MISSING"
fi

RP_MISSING=""
for f in register_complete auth_complete; do
  b=$(fnbody "$PK_C" "$f")
  hasf "$b" 'RP ID hash mismatch' || RP_MISSING="$RP_MISSING $f"
done
if [ -z "$RP_MISSING" ]; then
  ok "G3 (context) both doors still compare the RP ID hash"
else
  bad "G3 door(s) stopped comparing the RP ID hash:$RP_MISSING"
fi

# G4 — THE DELIBERATE NON-SHIP. The UV bit is still unchecked and passkey login
# still skips 2FA. Pinned so the next reader finds the reasoning attached to the
# fact rather than "helpfully" flipping the literal, which is retroactive and
# locks out every PIN-less roaming key already enrolled.
if hasf "$AUTH_BODY" 'issue_session_pub' && ! has "$PK_C" 'flags & 0x04'; then
  ok "G4 (context) UV is still unverified and passkey login still skips 2FA — deliberate, see the header; needs a uv column + a virtual-authenticator harness"
else
  bad "G4 the UV posture changed without this pin being updated — if a UV check landed, it needs a grandfathering column or it locks out enrolled PIN-less keys"
fi

echo
echo "=============================================="
printf '  PASS %d   FAIL %d\n' "$PASS" "$FAIL"
echo "=============================================="
[ "$FAIL" -eq 0 ]
