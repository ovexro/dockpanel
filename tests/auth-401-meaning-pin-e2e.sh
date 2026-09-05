#!/usr/bin/env bash
# auth-401-meaning-pin-e2e.sh — s329
#
# ONE PROPERTY: a 401 says which of two different things it means, and the
# client acts on the difference instead of guessing.
#
# Until v2.87.0 `api.ts` intercepted every 401, navigated to `/login`, and threw
# a fixed "Unauthorized" BEFORE the response body was ever read. Two harms, and
# the second is the one nobody had written down:
#
#   §B  THE BODY WAS DESTROYED. Every sentence written at a 401 site in this
#       codebase reached the user as the word "Unauthorized" — "Invalid
#       credentials" on the login form of every install, "Current password is
#       incorrect" on the account card, and "Two-factor authentication is no
#       longer set up on this account. Sign in again.", which is the ONE message
#       an administrator whose 2FA was reset by another admin needs to read.
#       Two of those sentences were written by s326 as a lost-authenticator
#       repair and no user has ever seen them.
#
#   §A  THE REDIRECT FIRED ON THE WRONG HALF. A 401 means either "your session
#       is gone" or "the credential you just typed was wrong", and only the
#       backend can tell them apart. The rule is structural, not a matter of
#       taste: IF A HANDLER RETURNED 401, THE SESSION WAS VALID, because the
#       `AuthUser` extractor had already let the request through. So session
#       death is minted in exactly one place and marked; a handler's 401 is
#       never marked, and never logs anyone out.
#
#   §C  THE LOGIN DOOR ANSWERED A CREDENTIAL QUESTION WITH A 500. An account
#       created through an identity provider has an empty `password_hash`, which
#       is not a PHC string, so `PasswordHash::new` failed and returned a 500 —
#       above `record_login_attempt`, so the probe was neither rate-limited nor
#       written to the activity log that `GET /api/security/login-audit` reads.
#       A non-existent address answered 401 and a password account answered 401;
#       the one class that answered anything else was "this address exists and
#       signs in with SSO". Unauthenticated, unthrottled, unlogged enumeration.
#
#   §D  CONTEXT — arms green at BOTH tags, so a harness measuring nothing cannot
#       read as a pass. Each owes its own mutation.
#
# ⚠ THE HONEST BOUND, pinned by D3 rather than hidden: the redirect now depends
# on a marker the backend emits, so a client newer than its backend (the only
# real window is a `scripts/update.sh` rollback, which restores the three
# binaries and has no frontend backup to restore) will not bounce a genuinely
# dead session — it shows the message instead. Nothing is exposed by that: the
# session is dead server-side and every request still 401s. Any full page load
# re-bootstraps through `/api/auth/me` and lands the user at `/login`.
#
# RUN AGAINST v2.86.0: every arm outside §D is RED there.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  What a 401 means — source pins (s329)"
echo "=============================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# Strip comments before matching. This ship writes long comments into every file
# it pins, and those comments quote the very sentences and tokens the arms look
# for, so an arm over RAW source would be satisfied by its own prose and an
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
cntre(){ grep -cE -- "$2" <<< "$1"; }

# 1-based line of the FIRST match, or 0. Used by the ORDER arms, which are the
# only ones immune to how a check is spelled.
lineof() { grep -nF -m1 -- "$2" <<< "$1" | cut -d: -f1 | grep -E '^[0-9]+$' || echo 0; }
lineofre(){ grep -nE -m1 -- "$2" <<< "$1" | cut -d: -f1 | grep -E '^[0-9]+$' || echo 0; }

# A function body bounded on ITS OWN braces, anchored on `fn NAME(` so a rename
# to a longer name cannot survive on the prefix. ⚠ An extraction that comes back
# EMPTY must be an ERROR, never an absence: an absence arm over an empty string
# is green on every possible tree.
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

# Production code only. The unit tests below `#[cfg(test)]` assert on the very
# constants the containment arms count, and a scanner that cannot tell a guard
# from a test convicts the test.
prod() { sed '/#\[cfg(test)\]/q' <<< "$1"; }

AUTH_RS=panel/backend/src/auth.rs
ERR_RS=panel/backend/src/error.rs
ROUTES_AUTH=panel/backend/src/routes/auth.rs
API_TS=panel/frontend/src/api.ts

AUTH_C=$(prod "$(code "$AUTH_RS")")
ERR_C=$(prod "$(code "$ERR_RS")")
ROUTES_C=$(code "$ROUTES_AUTH")
API_C=$(code "$API_TS")

if [ -z "$AUTH_C" ] || [ -z "$ERR_C" ] || [ -z "$ROUTES_C" ] || [ -z "$API_C" ]; then
  bad "FATAL a subject read or stripped EMPTY — every arm below would be vacuous"
  echo; printf '  PASS %d   FAIL %d\n' "$PASS" "$FAIL"; exit 1
fi
ok "A0 all four subjects extracted (auth.rs $(wc -l <<< "$AUTH_C")L, error.rs $(wc -l <<< "$ERR_C")L, routes/auth.rs $(wc -l <<< "$ROUTES_C")L, api.ts $(wc -l <<< "$API_C")L)"

echo
echo "§A session death is minted in ONE place, and named"

if has "$ERR_C" 'pub const CODE_SESSION_INVALID'; then
  ok "A1 the backend has a stable marker for 'you have no usable session'"
else
  bad "A1 no CODE_SESSION_INVALID in error.rs — the client is back to inferring a cause from a bare status"
fi

if has "$AUTH_C" 'fn session_invalid\('; then
  ok "A2 every session-death refusal is minted through one named function"
else
  bad "A2 no session_invalid() in auth.rs — 'which 401s mean log in again' is a property to re-derive from call sites again"
fi

# MEMBERSHIP, not a count. A threshold equal to its subject's own count sits at
# its own floor: it can neither absorb a deletion nor see an addition (#317).
# Each of the five sentences the extractor can refuse with is named here, so
# deleting any ONE of them is visible.
SENTENCES=(
  "session_invalid(\"Authentication required\")"
  "session_invalid(\"Invalid or expired token\")"
  "session_invalid(\"Token has been revoked\")"
  "session_invalid(\"Session revoked. Please log in again.\")"
  "session_invalid(\"Authentication required when X-Server-Id is provided\")"
)
MISSING=""
for s in "${SENTENCES[@]}"; do
  hasf "$AUTH_C" "$s" || MISSING="$MISSING\n      $s"
done
if [ -z "$MISSING" ]; then
  ok "A3 all ${#SENTENCES[@]} session-death refusals are marked (membership, not a threshold)"
else
  bad "A3 a session-death refusal is no longer marked — a dead session will not send the user to /login:$(printf "$MISSING")"
fi

# CONTAINMENT. After the fix, and after s467's authenticate_api_key(), the only
# literal UNAUTHORIZEDs left in auth.rs are the helper's own, the two
# agent-token refusals, and the three api_keys refusals — all five deliberately
# unmarked: a `dp_` key is the same session-less credential class as an agent
# token, presented fresh on every request, not a browser session that can be
# "lost". A new one appearing here SHOULD fail this arm and be classified by
# hand, same as it did for these three.
N_UNAUTH=$(cnt "$AUTH_C" 'StatusCode::UNAUTHORIZED')
if [ "$N_UNAUTH" -eq 6 ]; then
  ok "A4 auth.rs spells UNAUTHORIZED exactly 6 times (the helper + 2 agent-token + 3 api_keys refusals)"
else
  bad "A4 auth.rs spells UNAUTHORIZED $N_UNAUTH times, expected 6 — a new 401 here is either unmarked session death or a marked credential refusal; classify it"
fi

if hasf "$AUTH_C" 'err(StatusCode::UNAUTHORIZED, "Missing authorization")' \
   && hasf "$AUTH_C" 'err(StatusCode::UNAUTHORIZED, "Invalid token")'; then
  ok "A5 (context) the two agent-token refusals stay UNMARKED — an agent cannot make the client log a human out"
else
  bad "A5 the agent-token refusals changed shape — if they now carry the marker, a stale agent token logs every operator out"
fi

# MEMBERSHIP for the three api_keys refusals, same discipline as SENTENCES
# above for the marked ones — a threshold here could absorb one being deleted
# while a different one was added by mistake.
API_KEY_UNAUTH=(
  'err(StatusCode::UNAUTHORIZED, "Invalid or expired token")'
  'err(StatusCode::UNAUTHORIZED, "Key revoked. Mint a new one.")'
)
MISSING=""
for s in "${API_KEY_UNAUTH[@]}"; do
  hasf "$AUTH_C" "$s" || MISSING="$MISSING\n      $s"
done
# "Invalid or expired token" is deliberately spelled twice (key-not-found,
# owning-user-not-found) — both unmarked, both the same sentence a JWT decode
# failure ALSO uses (marked, via session_invalid, elsewhere in this file). Two
# different marking states behind one sentence is intentional, not a
# collision: the frontend only branches on the marker, never on the text.
N_INVALID_OR_EXPIRED=$(cnt "$AUTH_C" 'err(StatusCode::UNAUTHORIZED, "Invalid or expired token")')
if [ -z "$MISSING" ] && [ "$N_INVALID_OR_EXPIRED" -eq 2 ]; then
  ok "A5b the three api_keys refusals (2x invalid/expired + 1x revoked) are present and UNMARKED"
else
  bad "A5b an api_keys refusal is missing, miscounted, or was marked:$(printf "$MISSING")"
fi

# The invariant's teeth: a handler must never mint this. If a route module can
# claim session death, "a 401 from a handler proves the session was accepted"
# stops being true and the redirect is a guess again.
MINTERS=$(grep -rlF 'CODE_SESSION_INVALID' panel/backend/src --include=*.rs 2>/dev/null | sort | tr '\n' ' ')
MINTERS=$(echo $MINTERS)
if [ "$MINTERS" = "$ERR_RS $AUTH_RS" ] || [ "$MINTERS" = "$AUTH_RS $ERR_RS" ]; then
  ok "A6 the marker exists in exactly two files — where it is defined and where it is minted ($MINTERS)"
else
  bad "A6 the marker appears in [$MINTERS], expected only error.rs + auth.rs — a handler minting it breaks the rule that a handler 401 proves the session was accepted"
fi

echo
echo "§B the client reads the body, and only redirects when told to"

# THE ORDER ARM. Immune to how the guard is spelled: whatever the redirect
# condition becomes, the body must be parsed before the 401 branch is entered,
# or there is nothing to read it from. This is the arm that survives a refactor.
L_PARSE=$(lineofre "$API_C" 'const data = await res\.json\(\)')
L_401=$(lineofre "$API_C" 'if \(res\.status === 401\)')
if [ "$L_PARSE" -gt 0 ] && [ "$L_401" -gt 0 ] && [ "$L_PARSE" -lt "$L_401" ]; then
  ok "B1 the response body is parsed BEFORE the 401 branch (parse L$L_PARSE < branch L$L_401)"
else
  bad "B1 the 401 branch runs at L$L_401 and the body is parsed at L$L_PARSE — a 401 that returns before the parse destroys every sentence the backend wrote"
fi

BLOCK=$(awk '
  /if \(res\.status === 401\)/ { started=1 }
  started {
    n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
    if (opened || n>0) opened=1
    if (opened && depth<=0) exit
  }
' <<< "$API_C")

if [ -z "$BLOCK" ]; then
  bad "B2 the 401 block extracted EMPTY — B2/B3/B4 below would be vacuous"
else
  ok "B2 the 401 block extracted ($(wc -l <<< "$BLOCK")L)"

  # ORDER again, inside the block: the marker must be consulted before the
  # navigation, which is what makes the navigation conditional on it.
  L_MARK=$(lineof "$BLOCK" 'CODE_SESSION_INVALID')
  L_GO=$(lineof "$BLOCK" 'window.location.href = "/login"')
  if [ "$L_MARK" -gt 0 ] && [ "$L_GO" -gt 0 ] && [ "$L_MARK" -lt "$L_GO" ]; then
    ok "B3 the redirect is gated on the backend's marker (marker L$L_MARK < navigate L$L_GO)"
  else
    bad "B3 the 401 branch navigates to /login at L$L_GO without consulting the marker (L$L_MARK) — mistyping a password logs the user out again"
  fi

  if has "$BLOCK" 'error\?: string \}\)\.error \|\|'; then
    ok "B4 the thrown error carries the backend's own sentence"
  else
    bad "B4 the 401 branch no longer takes its message from the body — the user is back to reading the word 'Unauthorized' for six different causes"
  fi
fi

# NEGATIVE: the old unconditional throw must be gone from the CODE. The comment
# above it quotes that exact line on purpose, which is why this reads stripped
# source only.
if hasf "$API_C" 'throw new ApiError(401, "Unauthorized");'; then
  bad "B5 api.ts still throws a fixed 'Unauthorized' with no body — that IS the defect"
else
  ok "B5 no fixed-string 401 throw remains"
fi

# The public status page is a page the SPA serves to people with no account,
# during the outage that made them look it up. ServerProvider mounts OUTSIDE
# BrowserRouter, so its /servers fetch runs there too and 401s; without /status
# in this set the visitor is navigated to a login form.
if has "$API_C" 'const PUBLIC_PATHS' && hasf "$API_C" '"/status"'; then
  ok "B6 /status is a public path — an anonymous visitor is not bounced to /login"
else
  bad "B6 /status is not in the public path set — the public status page redirects the public to a login form"
fi

echo
echo "§C the login door cannot answer a credential question with a 500"

LOGIN_BODY=$(fnbody "$ROUTES_C" "login")
if [ -z "$LOGIN_BODY" ]; then
  bad "C0 the login body extracted EMPTY — every arm in §C would be vacuous"
else
  ok "C0 the login handler extracted ($(wc -l <<< "$LOGIN_BODY")L)"

  # ORDER: emptiness is tested BEFORE the parse. That order is the whole fix —
  # the 500 happened because the parse ran first on a string that cannot parse.
  L_EMPTY=$(lineof "$LOGIN_BODY" 'password_hash.is_empty()')
  L_PARSE2=$(lineof "$LOGIN_BODY" 'PasswordHash::new(&u.password_hash)')
  if [ "$L_EMPTY" -gt 0 ] && [ "$L_PARSE2" -gt 0 ] && [ "$L_EMPTY" -lt "$L_PARSE2" ]; then
    ok "C1 an absent password is detected before the hash is parsed (L$L_EMPTY < L$L_PARSE2)"
  else
    bad "C1 login parses the stored hash (L$L_PARSE2) without first testing it for emptiness (L$L_EMPTY) — an identity-provider account gets a 500, which tells a stranger the address exists and is SSO-only"
  fi

  # Both no-hash branches must spend the same Argon2 the real path spends, or
  # the status oracle just becomes a timing oracle.
  N_DUMMY=$(cnt "$LOGIN_BODY" 'PasswordHash::new(dummy_hash)')
  if [ "$N_DUMMY" -eq 2 ]; then
    ok "C2 both no-hash branches run the dummy verify (non-existent address, and identity-provider account)"
  else
    bad "C2 the dummy verify runs on $N_DUMMY of the 2 branches that have no hash to check — the branch that skips it returns in microseconds where every other outcome costs an Argon2 hash"
  fi

  # The failure path must be SHARED, not duplicated: a second `return 401`
  # written beside the first is how one of them ends up missing the two lines
  # that record and log the attempt.
  if has "$LOGIN_BODY" 'if !verified \{'; then
    ok "C3 a wrong password and an absent password leave through the SAME refusal"
  else
    bad "C3 the empty-hash branch has its own exit — if it does not pass through record_login_attempt and log_activity, the probe stays unthrottled and invisible to the login-audit surface"
  fi

  # The dummy hash is a module-level constant so a unit test can assert it
  # parses: the empty-hash branch calls `.expect()` on it, and a dummy that did
  # not parse would turn the enumeration fix into a panic on the same input.
  if hasf "$ROUTES_C" 'const DUMMY_PASSWORD_HASH' && has "$ROUTES_C" 'Argon2::default\(\)'; then
    ok "C4 the dummy hash is a named constant, so its validity is executable rather than assumed"
  else
    bad "C4 the dummy hash is inline again — nothing can assert it parses, and the branch that .expect()s it is the enumeration fix"
  fi
fi

echo
echo "§D context — green at BOTH tags by design; each owes its own mutation"

# The re-authentication challenge stays 403. It was chosen as a way AROUND this
# client (error.rs said so in as many words), and now that 401 bodies survive it
# is tempting to "restore" it. Don't: the session IS still valid there, and 403
# is what that means. This arm exists so that temptation is visible.
# ⚠ Scoped to the FUNCTION THAT BUILDS THE REFUSAL, not to the module. The first
# draft of this arm was `! has "$ROUTES_C" 'UNAUTHORIZED.*CODE_REAUTH_REQUIRED'`
# — a single-line regex against a status and a code that sit on DIFFERENT lines,
# so it could never match, the negation was trivially true, and the arm reduced
# to "the constant is still declared somewhere". Its mutation SURVIVED, which is
# how it was caught.
REAUTH_BODY=$(fnbody "$ROUTES_C" "reauth_required")
if [ -z "$REAUTH_BODY" ]; then
  bad "D1 reauth_required extracted EMPTY — this arm would be vacuous"
elif has "$ERR_C" 'pub const CODE_REAUTH_REQUIRED' \
   && hasf "$REAUTH_BODY" 'StatusCode::FORBIDDEN' \
   && ! hasf "$REAUTH_BODY" 'StatusCode::UNAUTHORIZED'; then
  ok "D1 (context) the re-auth challenge is still a 403, not a 401"
else
  bad "D1 the re-auth challenge became a 401 — it refuses one ACTION, not the session, and 403 is what that means"
fi

# The honest bound, pinned so it cannot be quietly forgotten: the marker is the
# ONLY redirect trigger, so a client newer than its backend does not bounce a
# dead session. If someone later adds a second trigger, this arm should be
# re-derived deliberately rather than drifted past.
N_TRIGGERS=$(cntre "$API_C" 'window\.location\.href = "/login"')
if [ "$N_TRIGGERS" -eq 1 ]; then
  ok "D3 (context) exactly one thing in the client navigates to /login"
else
  bad "D3 the client has $N_TRIGGERS navigations to /login — a second one is a second policy, and only one of them is pinned"
fi

echo
printf '  PASS %d   FAIL %d\n' "$PASS" "$FAIL"
echo
[ "$FAIL" -eq 0 ]
