#!/usr/bin/env bash
# twofa-recovery-pin-e2e.sh — s326
#
# ONE PROPERTY: losing your second factor is recoverable, and the doors that
# check that factor are all guarded to the same standard.
#
#   §A  AN ADMINISTRATOR CAN CLEAR SOMEBODY ELSE'S ENROLMENT.
#       Until this ship `twofa_disable` held the ONLY statement in the panel that
#       clears `totp_enabled`, and reaching it required a code — from the app you
#       lost, or from recovery codes that every pre-v2.83.0 enrolment holds but
#       was NEVER SHOWN (the block that draws them sat inside a branch the enable
#       handler cleared in the same breath). So for those accounts both factors
#       were gone at once and no panel path existed. ⚠ The route must REFUSE the
#       caller's own id: resetting your own 2FA from an admin screen would strip
#       the factor with no code presented at all, which is the one thing the
#       disable flow exists to prevent.
#
#   §B  AN OWNER CAN REPAIR THEMSELVES WITHOUT LOSING THE ENROLMENT.
#       Codes were written once and consumed one at a time, with no way to mint
#       more short of disabling 2FA entirely, and after ten recovery logins the
#       fallback was silently gone.
#
#   §C  EVERY DOOR THAT VERIFIES A 2FA CODE IS RATE-LIMITED.  ← the headline
#       The limiter lived INLINE inside the login handler and nowhere else, so it
#       guarded the login door alone. `twofa_disable` checks the same 6-digit code
#       against the same secret in order to switch the factor OFF, and could be
#       guessed without limit: `check_current` runs with skew 1, so 3 codes in
#       10^6 are live at any instant. Nothing sits above it either — the router
#       layers only CORS, timeout and tracing, and the `/api/` block `setup.sh`
#       writes for the panel vhost carries no `limit_req`.
#       Enumerated PER MEMBER, because comparing two totals is not a per-member
#       check and a mutation lived through exactly that at s324 (#292).
#
#   §D  THE COUNT IS VISIBLE.  Nothing ever reported how many codes were left.
#
#   §E  A REISSUED SET IS ACTUALLY DRAWN.  The v2.83.0 defect (E4 in the s325
#       suite) was a render block nested in a branch that was false by the time
#       the codes existed. The new path must not reintroduce it.
#
#   §F  THE GUIDE DESCRIBES WHAT THE CODE DOES.  It claimed Argon2 for a SHA-256
#       hash, and claimed enforcement prompts a user at their next login when
#       `twofa_required` is a response field with zero consumers anywhere.
#
#   §G  CONTEXT — arms that are green at BOTH tags, so a harness measuring
#       nothing cannot read as a pass. Each owes a mutation (s324 #292).
#
# RUN AGAINST v2.83.0: 28 arms RED, 5 green. The 5 are context, and each was
# separately killed by a mutation, because an arm nobody has ever seen fail is an
# arm nobody has tested (s324 #292):
#   G1 G2 G3  — controls, green at both tags by design.
#   E1        — the v2.83.0 fix itself; this suite guards it against regression.
#   B7        — "the field is as wide as the codes issued", which was TRUE at
#               v2.83.0 (8 and 8) and is true now (16 and 16). It is the *step*
#               that is pinned, not either number.
# ⚠ Do not read the header of a suite as evidence about the suite. B7's extractor
# read a constant 8 out of `[u8; 8]` — the 8 in `u8` — so it compared 16 against a
# hardcoded 16 and passed on every tree. Only the mutation that widened the
# generator to 32 bytes exposed it.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  2FA recovery + limiter — source pins (s326)"
echo "=============================================="
echo

PASS=0; FAIL=0; SKIP=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# Strip comments before matching, or an arm can be satisfied by the prose that
# NARRATES it — and this ship added comments to the very files it pins, each of
# which spells the defect it fixed.
code() {
  # A subject this ship CREATES does not exist at the previous tag. Returning
  # empty rather than erroring is what lets each arm be demonstrated RED there
  # individually, instead of the suite aborting on the first missing file and
  # proving only that it aborts (s325 #298).
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
    s{\{/\*.*?\*/\}}{}gs;
  ' "$1"
}

# Markdown is read RAW: prose is the subject there, not the noise.
raw() { [ -f "$1" ] && cat "$1" || true; }

has() { grep -qE -- "$2" <<< "$1"; }
hasf() { grep -qF -- "$2" <<< "$1"; }
flat() { tr '\n' ' ' <<< "$1" | tr -s ' '; }

# A function body, bounded on ITS OWN braces.
# ⚠ Anchored on `fn NAME(` and not the bare name: `twofa_disable` is a PREFIX of
# `twofa_disable_renamed`, and at s325 a rename mutation survived an arm that had
# left the terminator off (#299).
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

AUTH=panel/backend/src/routes/auth.rs
USERS=panel/backend/src/routes/users.rs
ROUTER=panel/backend/src/routes/mod.rs
CARDS=panel/frontend/src/components/AccountSecurity.tsx
USERPAGE=panel/frontend/src/pages/Users.tsx
GUIDE=docs/guides/security-hardening.md

AUTH_C=$(code "$AUTH")
USERS_C=$(code "$USERS")
ROUTER_C=$(code "$ROUTER")
CARDS_C=$(code "$CARDS")
USERPAGE_C=$(code "$USERPAGE")
GUIDE_R=$(raw "$GUIDE")

# ── §A the administrator's door ─────────────────────────────────────────────
echo "── §A an administrator can clear somebody else's enrolment ──"

RESET_BODY=$(fnbody "$USERS_C" "reset_2fa")

if [ -n "$RESET_BODY" ]; then
  ok "A1 a reset_2fa handler exists"
else
  bad "A1 no reset_2fa handler — a lost authenticator has no panel path back"
fi

# Its own body, not the file: users.rs is full of AdminUser handlers.
if has "$RESET_BODY" 'AdminUser'; then
  ok "A2 reset_2fa is AdminUser"
else
  bad "A2 reset_2fa is not AdminUser — clearing another account's 2FA is not an ordinary permission"
fi

# An unrouted handler is dead code that every other arm here would still pass.
if has "$ROUTER_C" 'users/\{id\}/reset-2fa'; then
  ok "A3 reset-2fa is routed"
else
  bad "A3 reset_2fa exists but nothing routes to it — the door is not connected"
fi

# ⚠ THE SECURITY ARM. Without this the route is a re-authentication bypass: an
# administrator (or a hijacked administrator session) turns 2FA off in one click,
# presenting no code at all.
if has "$RESET_BODY" 'id == claims\.sub'; then
  ok "A4 reset_2fa refuses the caller's own account"
else
  bad "A4 reset_2fa will act on the CALLER — that is disabling your own 2FA with no code, from an admin screen"
fi

# All three columns, not just the flag: leaving the secret behind leaves a
# half-enrolment that `twofa_setup` then refuses to replace.
A5_OK=1
for col in 'totp_enabled = FALSE' 'totp_secret = NULL' 'recovery_codes = NULL'; do
  hasf "$RESET_BODY" "$col" || A5_OK=0
done
if [ "$A5_OK" -eq 1 ]; then
  ok "A5 reset_2fa clears the flag, the secret and the codes"
else
  bad "A5 reset_2fa leaves part of the enrolment behind"
fi

if has "$RESET_BODY" 'revoke_all_user_sessions'; then
  ok "A6 reset_2fa signs the target's sessions out"
else
  bad "A6 reset_2fa leaves live sessions on an account whose authentication just changed"
fi

if has "$RESET_BODY" 'log_activity'; then
  ok "A7 reset_2fa is recorded in the activity log"
else
  bad "A7 reset_2fa is silent — an admin clearing another admin's 2FA leaves no trace"
fi

# The UI half: offered only where there is something to reset, and never on self.
if has "$USERPAGE_C" 'user\.totp_enabled' && has "$USERPAGE_C" 'isSelf\(user\.id\)'; then
  ok "A8 the control is offered only on enrolled rows, and not on your own"
else
  bad "A8 the reset control is shown on rows it cannot serve"
fi

# ...which requires the list to actually carry the field. A UI keyed on a field
# the API never sends is a control that never appears.
if has "$USERS_C" 'totp_enabled' && has "$(fnbody "$USERS_C" "list")" 'totp_enabled'; then
  ok "A9 the users list carries totp_enabled"
else
  bad "A9 the list never sends totp_enabled — A8's condition is always false and the control never draws"
fi

# ── §B the owner's own repair ───────────────────────────────────────────────
echo
echo "── §B an owner can reissue codes without losing the enrolment ──"

REGEN_BODY=$(fnbody "$AUTH_C" "twofa_regenerate_recovery_codes")

if [ -n "$REGEN_BODY" ]; then
  ok "B1 a recovery-code reissue handler exists"
else
  bad "B1 no reissue handler — codes are still write-once and cannot be replaced"
fi

if has "$ROUTER_C" '2fa/recovery-codes'; then
  ok "B2 the reissue route is wired"
else
  bad "B2 the reissue handler is not routed"
fi

# Scoped to its OWN body: `try_recovery_code` is called by two other handlers, so
# a file-wide grep passes on a tree where this one never learned about it.
if has "$REGEN_BODY" 'try_recovery_code'; then
  ok "B3 reissue accepts a recovery code, not only a live TOTP code"
else
  bad "B3 reissue takes only a TOTP code — the account it most helps is the one that cannot produce one"
fi

if has "$REGEN_BODY" 'generate_recovery_codes' && has "$REGEN_BODY" 'recovery_codes = \$1'; then
  ok "B4 reissue mints a new set and stores it"
else
  bad "B4 reissue does not actually write a new set"
fi

if has "$REGEN_BODY" 'recovery_codes.*codes'; then
  ok "B5 reissue returns the plaintext codes to the caller"
else
  bad "B5 reissue stores codes it never shows — the v2.83.0 defect, one route over"
fi

# A recovery code is a credential stored under an UNSALTED SHA-256, which is a
# fast hash — so its only real defence is its width. At 4 bytes the preimage space
# was 32 bits and a leaked `users` table gave up the plaintext in seconds.
if [ -n "$(grep -oE '\[u8; 8\]' <<< "$(fnbody "$AUTH_C" "generate_recovery_codes")")" ]; then
  ok "B6 recovery codes are 8 random bytes (64 bits), not 4"
else
  bad "B6 recovery codes are narrower than 8 bytes — a fast unsalted hash over a small space"
fi

# ⚠ CROSS-TREE, and DERIVED rather than spelled: the field that accepts a recovery
# code must be at least as wide as the code the backend issues. Widening the
# generator without widening the input reintroduces the v2.83.0 defect exactly —
# a code that cannot be typed — and both halves look correct in isolation, which
# is why this arm reads BOTH trees and compares them rather than asserting a
# number in either one.
# ⚠ CAPTURE the width. `grep -oE '[0-9]+' | head -1` over `[u8; 32]` returns the
# **8 in `u8`**, so this arm read a constant 8 no matter what the generator said —
# it passed on a clean tree, and only a mutation that widened the generator to 32
# bytes exposed it. Same defect as the cap extractor below, in the same commit,
# five lines apart; the one that reported a cap of 0 was caught instantly because
# it went red, and this one was not because it went green.
GEN_BYTES=$(sed -nE 's/.*\[u8; ([0-9]+)\].*/\1/p' <<< "$(fnbody "$AUTH_C" "generate_recovery_codes")" | head -1)
# ⚠ Extract the cap with a CAPTURE. `grep -oE '[0-9]+' | head -1` returns the
# ZERO in `slice(0, …`, and this arm's first run duly reported a cap of 0 against
# a field that was already correct.
UI_CAP=$(sed -nE 's/.*slice\(0, ([0-9]+)\).*/\1/p' <<< "$(grep -F 'e.target.value.replace(/[^0-9a-fA-F]/g' <<< "$CARDS_C")" | head -1)
if [ -n "$GEN_BYTES" ] && [ -n "$UI_CAP" ] && [ "$UI_CAP" -ge $((GEN_BYTES * 2)) ]; then
  ok "B7 the code field ($UI_CAP) is wide enough for the codes issued ($((GEN_BYTES * 2)))"
else
  bad "B7 the field caps at ${UI_CAP:-?} but the backend issues $(( ${GEN_BYTES:-0} * 2 ))-char codes — the code cannot be typed"
fi

# ── §C the limiter covers every door that checks a 2FA code ─────────────────
echo
echo "── §C every door that verifies a 2FA code is rate-limited ──"

# Discovery. The members are the handlers that treat a 2FA code as a CREDENTIAL —
# which is exactly the set that also accepts the recovery fallback. `twofa_enable`
# checks a code too, but against a secret the caller just generated and already
# holds, so it is deliberately not a member.
#
# Enumerated from source rather than listed, so a NEW door of this shape joins the
# check automatically instead of being silently uncovered.
MEMBERS=""
for fn in $(grep -oE 'pub async fn [a-z0-9_]+' "$AUTH" | awk '{print $4}'); do
  b=$(fnbody "$AUTH_C" "$fn")
  if has "$b" 'try_recovery_code\('; then
    MEMBERS="$MEMBERS $fn"
  fi
done
MEMBERS_N=$(printf '%s\n' $MEMBERS | grep -c . || true)

printf '%s\n' $MEMBERS | sed 's/^/  door: /'

# §C0 — the discovery itself. Three are known to exist (login's second step,
# disable, reissue). Fewer means the pattern moved and every arm below inspects
# nothing.
if [ "$MEMBERS_N" -ge 3 ]; then
  ok "C0 discovered $MEMBERS_N doors that verify a 2FA credential (>= 3 known)"
else
  bad "C0 only $MEMBERS_N discovered — the pattern moved; every §C arm is now vacuous"
fi

for fn in $MEMBERS; do
  b=$(fnbody "$AUTH_C" "$fn")
  if has "$b" 'twofa_rate_limit_check'; then
    ok "C-$fn is rate-limited"
  else
    bad "C-$fn verifies a 2FA code with NO limit — 3 codes in 10^6 are live at any instant"
  fi
done

# A door that never records a failure has a limiter that never counts.
for fn in $MEMBERS; do
  b=$(fnbody "$AUTH_C" "$fn")
  if has "$b" 'twofa_rate_limit_record'; then
    ok "C-$fn records failed attempts"
  else
    bad "C-$fn checks the limit but never increments it — the counter stays at zero forever"
  fi
done

# The limiter lives in ONE place. A second inline copy is how the first one came
# to guard a single door.
#
# ⚠ This arm first read `grep -c twofa_attempts <= 3`, and that was GREEN AT
# v2.83.0 — where all three touches were INLINE inside the login handler, which is
# the exact defect. A threshold satisfied by the bug is not a check. The property
# is CONTAINMENT: every touch must sit inside one of the three helpers, so the
# file-wide count and the in-helper count have to agree.
TOTAL_TOUCH=$(grep -c 'twofa_attempts' <<< "$AUTH_C" || true)
HELPER_TOUCH=0
HELPERS_PRESENT=1
for h in twofa_rate_limit_check twofa_rate_limit_record twofa_rate_limit_clear; do
  hb=$(fnbody "$AUTH_C" "$h")
  n=$(grep -c 'twofa_attempts' <<< "$hb" || true)
  [ "$n" -ge 1 ] || HELPERS_PRESENT=0
  HELPER_TOUCH=$((HELPER_TOUCH + n))
done
if [ "$HELPERS_PRESENT" -eq 1 ] && [ "$TOTAL_TOUCH" -eq "$HELPER_TOUCH" ]; then
  ok "C9 every touch of the limiter state is inside its three helpers ($HELPER_TOUCH/$TOTAL_TOUCH)"
else
  bad "C9 $((TOTAL_TOUCH - HELPER_TOUCH)) touch(es) of twofa_attempts sit outside the helpers — an inline copy will drift"
fi

# ── §D the remaining count is visible ───────────────────────────────────────
echo
echo "── §D the account can see how many codes are left ──"

if has "$(fnbody "$AUTH_C" "twofa_status")" 'recovery_codes_remaining'; then
  ok "D1 the status endpoint reports how many codes remain"
else
  bad "D1 nothing reports the remaining count — an account can reach zero without being told"
fi

if has "$CARDS_C" 'recovery_codes_remaining'; then
  ok "D2 the card reads the count"
else
  bad "D2 the endpoint reports a count with no reader"
fi

# Zero is its own state. "0 recovery codes remaining" as a neutral line is the
# wrong answer to the situation it describes.
if has "$CARDS_C" 'remaining === 0'; then
  ok "D3 the card treats zero as its own state"
else
  bad "D3 zero is rendered like any other number — the one value that means the fallback is gone"
fi

# ── §E a reissued set is drawn ──────────────────────────────────────────────
echo
echo "── §E the reissued codes are actually rendered ──"

# Same structural test as the s325 suite's E4, applied to the new path: the block
# that draws the codes must sit OUTSIDE the enabled/setup ternary, because both
# handlers that produce codes leave that ternary's condition false.
TERNARY=$(awk '
  index($0, "{enabled ? (") { s=1 }
  s { print; if ($0 ~ /^[[:space:]]{8}\)\}[[:space:]]*$/) exit }
' <<< "$CARDS_C")

if has "$CARDS_C" 'recoveryCodes\.length > 0' && ! has "$TERNARY" 'recoveryCodes\.length > 0'; then
  ok "E1 the codes render outside the branch their own handler clears"
else
  bad "E1 the codes are nested in a branch that is false by the time they exist"
fi

# The reissue button must feed that same block, or it succeeds and shows nothing.
REGEN_HANDLER=$(awk '
  index($0, "/auth/2fa/recovery-codes") { s=1 }
  s { print; n++ ; if (n > 12) exit }
' <<< "$CARDS_C")
if has "$REGEN_HANDLER" 'setRecoveryCodes'; then
  ok "E2 the reissue button feeds the render block"
else
  bad "E2 reissue returns codes the page never puts anywhere — silent success"
fi

# ── §F the guide describes what the code does ───────────────────────────────
echo
echo "── §F the guide describes what the code actually does ──"

# `hash_token` is SHA-256 (routes/auth.rs). The guide named Argon2 — a materially
# stronger protection than the one applied, in the document an operator reads to
# assess exactly that.
if ! has "$GUIDE_R" 'recovery codes are stored as Argon2' && ! has "$GUIDE_R" 'Recovery codes are stored as Argon2'; then
  ok "F1 the guide no longer claims Argon2 for the recovery codes"
else
  bad "F1 the guide still claims Argon2 — the codes are hashed with SHA-256"
fi

# The control's own reader: `twofa_required` is set on the login response and read
# by nothing, so enforcement warns and never refuses.
if ! has "$GUIDE_R" 'prompted to set it up on their next login'; then
  ok "F2 the guide no longer promises a login prompt nothing implements"
else
  bad "F2 the guide says enforcement prompts at next login — twofa_required has no consumer"
fi

# The limit that cannot be engineered away is the one that must be written down.
if has "$GUIDE_R" 'sole administrator'; then
  ok "F3 the guide states the case the reset route cannot serve"
else
  bad "F3 the guide sells a recovery story without naming who it still fails"
fi

# ── §G context — green at BOTH tags, so a vacuous harness cannot read green ──
echo
echo "── §G context (green at both tags — each owes a mutation) ──"

if [ -n "$(fnbody "$USERS_C" "reset_password")" ]; then
  ok "G1 reset_password still exists — §A's sibling, and the shape it mirrors"
else
  bad "G1 reset_password is gone — §A is measuring an admin module that changed underneath it"
fi

if has "$(fnbody "$AUTH_C" "twofa_verify")" 'try_recovery_code'; then
  ok "G2 the login door still accepts a recovery code (fnbody is discriminating)"
else
  bad "G2 twofa_verify lost its recovery branch — or fnbody is broken and §C's discovery is empty"
fi

ROUTES_N=$(grep -c '\.route(' <<< "$ROUTER_C" || true)
if [ "$ROUTES_N" -ge 200 ]; then
  ok "G3 the router still parses as a router ($ROUTES_N routes) — §A3/§B2 are non-vacuous"
else
  bad "G3 only $ROUTES_N routes read from the router — the file shape moved"
fi

echo
echo "----------------------------------------------"
printf "  PASS %d  FAIL %d  SKIP %d\n" "$PASS" "$FAIL" "$SKIP"
echo "----------------------------------------------"
echo

[ "$FAIL" -eq 0 ]
