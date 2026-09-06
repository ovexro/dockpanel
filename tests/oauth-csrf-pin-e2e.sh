#!/usr/bin/env bash
# oauth-csrf-pin-e2e.sh — s418 / v2.171.0
#
# Pins two findings from the s418 identity/access-control audit fan-out (finder
# + adversarial skeptic, both UPHELD independently against source and, for §A,
# a live production reachability check — see project_dockpanel_lessons_p154):
#
#   §A  /authorize's `state` token is bound to the ISSUING BROWSER, not just
#       proven a member of the server-side map. Before this fix, `state`'s only
#       property was "some /authorize call produced it" — nothing tied the
#       browser presenting it at /callback to the browser that received it.
#       Login-CSRF: an attacker starts their own OAuth flow, captures the
#       provider's redirect back to this panel (a real code+state pair for the
#       ATTACKER's own account) without letting their own browser follow it,
#       and hands that exact callback URL to a victim. The old code validated
#       `state` purely against `oauth_states` — which the attacker's `state`
#       legitimately is a member of — and would have logged the victim's
#       browser into the ATTACKER's account.
#
#   §B  An OAuth provider's `email_verified: false` (or equivalent) cannot walk
#       straight into the account-linking/auto-login logic. `oauth.rs`'s
#       existing-account branches link or log in by EMAIL MATCH ALONE once a
#       provider hands back an email; nothing checked whether the provider
#       itself considers that email verified. Google's OIDC userinfo endpoint
#       (the one this panel calls) does return `email_verified` and it was
#       never read.
#
# Pure source analysis: no box, no network, no build.
#
# NO PIPES INTO `grep -q` — under `set -o pipefail` grep -q closes the pipe on
# its first match and the arm goes red on correct code, non-deterministically.
# Every arm feeds grep a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

OAUTH=panel/backend/src/routes/oauth.rs

[ -f "$OAUTH" ] || bad "MISSING SUBJECT FILE: $OAUTH"

# Comments out, CODE INTACT — copied verbatim from status-page-gate-pin-e2e.sh's
# proven stripper (lesson #136): a naive multi-line strip deletes real code
# whenever `/*` occurs inside a string literal, and a truncated subject makes an
# ABSENCE arm pass on code the stripper merely removed.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }

# The body of one top-level fn, bounded by the NEXT top-level fn. A fixed
# `grep -A n` window is not a function (lesson #131/#172).
fnbody() {
  awk -v name="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / {
      if ($0 ~ "(pub |pub\\(crate\\) )?(async )?fn " name "\\(") { inside=1; next }
      inside=0
    }
    inside { print }
  ' <<< "$1"
}

# The DECLARATION of one top-level fn: its line plus the parameter list, up to
# the return arrow. Stops before the body so it can never read a call inside the
# function as if it were an extractor.
fnsig() {
  awk -v name="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / {
      inside = ($0 ~ "(pub |pub\\(crate\\) )?(async )?fn " name "\\(")
    }
    inside { print }
    inside && /\)[[:space:]]*->/ { inside=0 }
  ' <<< "$1"
}

# Byte offset of the first match of a pattern inside a string — used to assert
# ORDER between two anchors in a function body, not just their co-presence. An
# arm that only checks "both strings appear somewhere" cannot tell "the gate
# runs before the lookup" from "the gate is dead code after it".
first_offset() {
  grep -boE -- "$2" <<< "$1" | head -1 | cut -d: -f1
}

echo "== §A  /authorize's state is bound to the issuing browser (login-CSRF) =="

if S=$(subj "$OAUTH"); then
  AUTHSIG=$(fnsig "$S" authorize)
  AUTHBODY=$(fnbody "$S" authorize)
  CALLSIG=$(fnsig "$S" callback)
  CALLBODY=$(fnbody "$S" callback)

  # Floor the four subjects first. An fnbody/fnsig whose pattern stops matching
  # yields an empty subject, and every arm below it would then pass green for a
  # file that no longer contains the code at all.
  if [ "${#AUTHSIG}" -ge 80 ] && [ "${#AUTHBODY}" -ge 400 ] \
  && [ "${#CALLSIG}" -ge 80 ] && [ "${#CALLBODY}" -ge 800 ]; then
    ok "A0 function subjects resolved (authorize sig ${#AUTHSIG}c/body ${#AUTHBODY}c, callback sig ${#CALLSIG}c/body ${#CALLBODY}c)"

    if grep -qE 'headers: *axum::http::HeaderMap' <<< "$AUTHSIG"; then
      ok "A1 authorize() takes the request headers (needed to set the Secure flag correctly)"
    else
      bad "A1 authorize() takes the request headers"
    fi

    if grep -qE 'oauth_csrf=\{csrf_state\}' <<< "$AUTHBODY" \
    && grep -qE 'HttpOnly' <<< "$AUTHBODY" \
    && grep -qE 'SameSite=Lax' <<< "$AUTHBODY"; then
      ok "A2 authorize() sets an HttpOnly, SameSite=Lax oauth_csrf cookie carrying the state value"
    else
      bad "A2 authorize() sets an HttpOnly, SameSite=Lax oauth_csrf cookie carrying the state value"
    fi

    if grep -qE 'header::SET_COOKIE' <<< "$AUTHBODY"; then
      ok "A3 authorize()'s response actually carries a Set-Cookie header (not just a formatted string that goes unused)"
    else
      bad "A3 authorize()'s response actually carries a Set-Cookie header"
    fi

    if grep -qE "strip_prefix\\(\"oauth_csrf=\"\\)" <<< "$CALLBODY"; then
      ok "A4 callback() reads the oauth_csrf cookie back out of the request"
    else
      bad "A4 callback() reads the oauth_csrf cookie back out of the request"
    fi

    if grep -qE 'csrf_cookie\.as_deref\(\) *!= *Some\(query\.state\.as_str\(\)\)' <<< "$CALLBODY"; then
      ok "A5 callback() rejects when the cookie is absent or does not match query.state"
    else
      bad "A5 callback() rejects when the cookie is absent or does not match query.state"
    fi

    # Order matters: the cookie check must be reachable on every path through
    # the handler, not stapled on after the point where a session could already
    # have been minted. Anchor against the final JWT-issuing line — the cookie
    # check must run BEFORE it exists at all in program order.
    o_check=$(first_offset "$CALLBODY" 'csrf_cookie\.as_deref')
    o_issue=$(first_offset "$CALLBODY" 'let token = jsonwebtoken::encode')
    if [ -n "$o_check" ] && [ -n "$o_issue" ] && [ "$o_check" -lt "$o_issue" ]; then
      ok "A6 the browser-binding check runs before a session can be issued, not after"
    else
      bad "A6 the browser-binding check runs before a session can be issued, not after"
    fi
  else
    bad "A0 function subjects resolved — extraction is broken, A1-A6 would be vacuous"
    bad "A1 authorize() takes the request headers"; bad "A2 oauth_csrf cookie set"
    bad "A3 Set-Cookie header present"; bad "A4 callback() reads the cookie"
    bad "A5 callback() rejects on mismatch"; bad "A6 check runs before session issuance"
  fi
fi

echo "== §B  an unverified provider email cannot auto-link or auto-login =="

if S=$(subj "$OAUTH"); then
  CALLBODY=$(fnbody "$S" callback)

  if [ "${#CALLBODY}" -ge 800 ]; then
    # NOT a bare 'email_verified' substring check: the users table has its own
    # unrelated email_verified COLUMN, named in this same function's INSERT —
    # a bare-word arm here is vacuous (matches even pre-fix code, verified by
    # mutation test). Require the specific call shape that reads it off the
    # PROVIDER's response.
    if grep -qE 'userinfo\.get\("email_verified"\)' <<< "$CALLBODY"; then
      ok "B1 callback() reads email_verified from the provider's userinfo response"
    else
      bad "B1 callback() reads email_verified from the provider's userinfo response"
    fi

    if grep -qE '\.and_then\(\|v\| v\.as_bool\(\)\)\.unwrap_or\(true\)' <<< "$CALLBODY"; then
      ok "B2 an ABSENT field is treated as verified (GitLab/GitHub-profile paths never send it — this must not become a new rejection)"
    else
      bad "B2 an absent email_verified field defaults to allow, not deny"
    fi

    if grep -qE 'if !email_verified' <<< "$CALLBODY" \
    && grep -qE 'not verified' <<< "$CALLBODY"; then
      ok "B3 an explicit email_verified=false is rejected with a real error, not silently ignored"
    else
      bad "B3 an explicit email_verified=false is rejected"
    fi

    # Order matters even more here than in §A: the gate is worthless if the
    # email has already been used to look up/link/create an account by the time
    # it runs. Anchor against the user lookup, the earliest point email trust
    # starts mattering.
    o_gate=$(first_offset "$CALLBODY" 'if !email_verified')
    o_lookup=$(first_offset "$CALLBODY" 'SELECT \* FROM users WHERE email = \$1')
    if [ -n "$o_gate" ] && [ -n "$o_lookup" ] && [ "$o_gate" -lt "$o_lookup" ]; then
      ok "B4 the verification gate runs BEFORE the user is looked up or created, not after"
    else
      bad "B4 the verification gate runs before the user lookup"
    fi
  else
    bad "B1 callback() subject resolved (${#CALLBODY}c) — B1-B4 would be vacuous"
    bad "B2 absent field defaults to allow"; bad "B3 explicit false is rejected"
    bad "B4 gate runs before user lookup"
  fi
fi

echo "== §C  a matching provider NAME does not bypass a mismatched provider account id =="
#
# s473 / p209 deferred item 5, re-verified then closed: oauth_id was captured
# on link/create and never consulted on a returning login — only the provider
# NAME was checked. oauth_id is the provider's own permanent account id and is
# only ever written alongside oauth_provider (never separately), so this gap
# meant an email changing hands AT THE PROVIDER (a departed employee's mailbox
# reassigned to a new hire's fresh account there) would silently hand the new
# person the departed employee's dockpanel account and role, because the two
# accounts share nothing the callback checked except the email string and the
# provider's short name.

if S=$(subj "$OAUTH"); then
  CALLBODY=$(fnbody "$S" callback)

  if [ "${#CALLBODY}" -ge 800 ]; then
    if grep -qE 'oauth_id\.as_deref\(\) *!= *Some\(oauth_id\.as_str\(\)\)' <<< "$CALLBODY"; then
      ok "C1 callback() compares the incoming provider account id against the stored oauth_id, not just the provider name"
    else
      bad "C1 callback() compares the incoming provider account id against the stored oauth_id"
    fi

    # Order matters: the oauth_id check must sit INSIDE the "provider name
    # already matches" arm — textually AFTER the provider-name-mismatch reject,
    # never before it (which would misfire on a genuinely different provider).
    o_provider_mismatch=$(first_offset "$CALLBODY" 'oauth_provider\.as_deref\(\) *!= *Some\(provider_name\.as_str\(\)\)')
    o_id_mismatch=$(first_offset "$CALLBODY" 'oauth_id\.as_deref\(\) *!= *Some\(oauth_id\.as_str\(\)\)')
    if [ -n "$o_provider_mismatch" ] && [ -n "$o_id_mismatch" ] && [ "$o_provider_mismatch" -lt "$o_id_mismatch" ]; then
      ok "C2 the oauth_id check sits in the 'same provider name' branch, after the provider-name check"
    else
      bad "C2 the oauth_id check sits in the 'same provider name' branch, after the provider-name check"
    fi

    # A mismatch must actually REJECT, not merely be observed. This handler had
    # exactly 2 CONFLICT returns before this fix (the different-provider reject
    # + the auto-create duplicate-email reject) and 3 after.
    conflict_count=$(grep -coE 'StatusCode::CONFLICT' <<< "$CALLBODY")
    if [ "$conflict_count" -ge 3 ]; then
      ok "C3 a third CONFLICT reject exists (the id-mismatch branch actually returns an error)"
    else
      bad "C3 a third CONFLICT reject exists (found $conflict_count, need >= 3)"
    fi

    # The whole fix depends on oauth_provider never being written without
    # oauth_id in the SAME statement — split them and a legitimate returning
    # user starts failing C1's own check. Pin both writers.
    if grep -qE 'UPDATE users SET oauth_provider = \$1, oauth_id = \$2' <<< "$CALLBODY"; then
      ok "C4 the auto-link UPDATE still writes oauth_provider and oauth_id together"
    else
      bad "C4 the auto-link UPDATE writes oauth_provider and oauth_id together"
    fi

    if grep -qE 'INSERT INTO users \(email, password_hash, role, email_verified, oauth_provider, oauth_id, approved\)' <<< "$CALLBODY"; then
      ok "C5 the auto-create INSERT still writes oauth_provider and oauth_id together"
    else
      bad "C5 the auto-create INSERT writes oauth_provider and oauth_id together"
    fi

    o_issue=$(first_offset "$CALLBODY" 'let token = jsonwebtoken::encode')
    if [ -n "$o_id_mismatch" ] && [ -n "$o_issue" ] && [ "$o_id_mismatch" -lt "$o_issue" ]; then
      ok "C6 the oauth_id check runs before a session can be issued, not after"
    else
      bad "C6 the oauth_id check runs before a session can be issued, not after"
    fi
  else
    bad "C1 callback() subject resolved (${#CALLBODY}c) — C1-C6 would be vacuous"
    bad "C2 order: oauth_id check after provider-name check"
    bad "C3 a third CONFLICT reject exists"; bad "C4 UPDATE writes both columns together"
    bad "C5 INSERT writes both columns together"; bad "C6 check runs before session issuance"
  fi
fi

echo
printf 'oauth-csrf: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
