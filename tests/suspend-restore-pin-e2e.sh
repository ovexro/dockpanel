#!/usr/bin/env bash
# suspend-restore-pin-e2e.sh — s316 / v2.73.0
#
# THE SENTENCE THIS SUITE EXISTS TO PROVE, written out so a mutation can be
# aimed at it rather than at whatever the arms happen to catch (lesson #237 —
# §B of the transfer pin was mutation-tested by DELETING its predicate when it
# existed to catch a WIDENING, so it was green through the very change it was
# written for, and the ledger recorded that test as proof):
#
#   "The role an account held before it was suspended is recorded in a column
#    nothing else writes, and an un-suspend never returns a role more
#    privileged than the one that was recorded."
#
# Both halves matter and they fail differently. The FIRST half broke because
# `users.reset_token` was the stash AND the password-reset token: a suspended
# account could erase the record of its own role from the public forgot-password
# form, unauthenticated, without even completing the reset. The SECOND half broke
# because both restore paths guessed, and both guessed "user" — which is exactly
# the role that may bring a new domain into service, i.e. the one capability the
# `client` role exists to deny.
#
# So the mutations this suite must survive are WIDENING mutations — put a role
# back into the shared column, or make a fallback more privileged — not deletions.
# Every arm below was mutation-tested to redden ALONE.
#
#   §A  the stash has a column of its own, and the migration is guarded
#   §B  nothing but the password-reset flow writes `reset_token`
#   §C  the restore fails toward the SMALLEST capability
#   §D  the password-reset doors refuse a suspended account — silently
#   §E  one statement each, not a private copy per caller
#
# Pure source analysis: no box, no network, no build.
#
# NO PIPES INTO `grep -q`. Under `set -o pipefail` grep -q closes the pipe on its
# first match, the upstream dies of SIGPIPE (141), and pipefail reports the whole
# pipeline failed — so an arm goes red on correct code, non-deterministically.
# Every arm here feeds grep a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

HELPERS=panel/backend/src/helpers.rs
USERS=panel/backend/src/routes/users.rs
AUTH=panel/backend/src/routes/auth.rs
WHMCS=panel/backend/src/routes/whmcs.rs
MODELS=panel/backend/src/models.rs
MIGDIR=panel/backend/migrations

for f in "$HELPERS" "$USERS" "$AUTH" "$WHMCS" "$MODELS"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT.
#
# The obvious `s{/\*.*?\*/}{}gs` is wrong and shipped in four suites before the
# fix: `/*` occurs INSIDE string literals, so it opened a "block comment" that ran
# to the next `*/` and deleted real code. A truncated subject makes an ABSENCE arm
# pass on code the stripper merely removed, which is the worst way to be wrong.
# So a block comment is only recognised where one is actually written.
#
# Stripping comments is load-bearing here for a second reason: this suite asserts
# the ABSENCE of strings that the fix's own explanatory comments legitimately
# discuss (`feedback_source_pin_prose_trap` — a pin greps raw source, so it reads
# the prose that narrates the check as though it were the check).
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

# An arm whose subject could not be extracted must SKIP, not print a confident
# green next to a red about the same subject.
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }

has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }

# Strip `--` comments from SQL. Same reason as `code()`: an arm that greps a
# migration for `ADD COLUMN` would otherwise be satisfied by a comment
# discussing one.
sqlcode() { perl -pe 's{--.*$}{}' "$1"; }

# `grep -l` over CODE, not over raw bytes.
#
# ⚠ THE TRAP THIS EXISTS FOR. `grep -rl` reads a file as written, so it matches
# the prose that NARRATES a call exactly as readily as the call itself. Four arms
# in the first draft of this suite used `grep -rl` directly, and E1 stayed green
# with every helper call deleted from `users.rs` because two doc comments there
# still spelled `helpers::suspend_account`. This suite built a comment stripper
# and then did not use it — `feedback_source_pin_prose_trap`, inside the file
# written to avoid it. Every census below strips first.
files_matching() {
  local pat="$1"; shift
  local f out=""
  for f in "$@"; do
    [ -f "$f" ] || continue
    if grep -qE -- "$pat" <<< "$(code "$f")"; then out="$out$f "; fi
  done
  printf '%s' "$out"
}

BACKEND_RS=$(find panel/backend/src -name '*.rs' | sort)

# One function's body, bounded by its OWN closing brace — a `}` in column 0 —
# and never by a comment or a fixed line offset.
#
# ⚠ The first draft of D3 ended its range on a `// ───` banner that `code()` had
# already stripped, so the window ran to end-of-file and matched an unrelated
# `role == "suspended"` in the 2FA handler: green with the guard deleted. Caught
# by mutation, which is the only reason this comment can be written. A token
# search scoped to the wrong window is the same false green as one scoped to a
# whole file (#234, #172, #230, #237).
fn_body() { awk -v pat="$1" '$0 ~ pat {f=1} f {print} f && /^\}/ {exit}' <<< "$2"; }

HELPERS_S=$(subj "$HELPERS" || true)
USERS_S=$(subj "$USERS" || true)
AUTH_S=$(subj "$AUTH" || true)
WHMCS_S=$(subj "$WHMCS" || true)
MODELS_S=$(subj "$MODELS" || true)

echo "§A  the stash has a column of its own, and the migration is guarded"

# A1 — the column exists. Derived from the migration directory rather than from a
# hardcoded filename, so renaming the migration does not silently stop measuring.
MIG=""
for m in "$MIGDIR"/*.sql; do
  [ -f "$m" ] || continue
  if grep -qE 'ADD COLUMN IF NOT EXISTS prior_role' <<< "$(sqlcode "$m")"; then MIG="$m"; break; fi
done
if [ -z "$MIG" ]; then
  bad "A1 no migration adds users.prior_role — the stash has no column of its own"
else
  ok "A1 a migration adds users.prior_role ($(basename "$MIG"))"
fi

# A2 — THE BOOT ARM. `role` is varchar(20); `reset_token` is varchar(255) and
# holds a 64-char SHA-256 digest. An unguarded `prior_role = reset_token` fails
# with "value too long for type character varying(20)" on any install carrying a
# live or stale reset hash — and `sqlx::migrate!` runs at startup, so a failed
# migration is a panel that does not come up. The guard is also what stops the
# migration GUESSING a role for a row whose stash was already destroyed, which is
# the defect it exists to remove.
if [ -z "$MIG" ]; then
  : # already reported by A1
elif ! has "$(sqlcode "$MIG")" 'prior_role[[:space:]]*=[[:space:]]*reset_token'; then
  ok "A2 the migration does not copy reset_token into prior_role at all"
elif has "$(sqlcode "$MIG")" "reset_token IN \\('admin', 'reseller', 'user', 'client'\\)"; then
  ok "A2 the backfill copies only exact role values — a 64-char token cannot overflow varchar(20), and a destroyed stash is left NULL rather than guessed"
else
  bad "A2 the backfill copies reset_token unguarded — it will exceed varchar(20) on any install holding a reset hash, and sqlx::migrate! runs at startup"
fi

# A3 — `prior_role` must NOT join `models::User`. That struct derives FromRow and
# is loaded with `SELECT *` by the queries that authenticate a login; a database
# restored from a snapshot older than the migration would then fail EVERY login
# rather than degrading the one endpoint that reads the stash.
if [ -z "$MODELS_S" ]; then
  bad "A3 could not read $MODELS"
elif [ -z "$MIG" ]; then
  # Without the column this absence arm is true for the wrong reason. It was the
  # ONLY arm green at the previous tag, which is precisely the tell.
  : # already reported by A1
elif has "$MODELS_S" 'prior_role'; then
  bad "A3 prior_role is a field on models::User — a pre-migration schema would break every SELECT * login query, not just the restore"
else
  ok "A3 prior_role is read by a narrow SELECT, never through models::User"
fi

echo "§B  nothing but the password-reset flow writes reset_token"

# B1 — THE HEADLINE ABSENCE ARM, and the one a widening must redden.
#
# ⚠ Keyed on the COLUMN, not on `SET reset_token`. The first draft matched the
# latter and was green against `SET updated_at = NOW(), reset_token = $1` —
# i.e. it only reddened for a regression written in the same word order as the
# code that was deleted, which is the lesson-#237 trap this suite's own header
# warns about, in the arm the header calls the headline. `users.rs` has no
# business naming the password-reset column at all, in any statement, in any
# order, so that is what is asserted.
if [ -z "$USERS_S" ]; then
  bad "B1 could not read $USERS"
elif has "$USERS_S" 'reset_token'; then
  bad "B1 users.rs mentions reset_token again — the suspend stash is back in the password-reset column"
else
  ok "B1 users.rs never touches reset_token"
fi

# B2 — the same absence over the WHOLE backend, so a copy in a module nobody
# thought of is caught too. Only `auth.rs` (the reset flow) and `models.rs` (the
# struct field) may name it. Stripped, so a comment discussing the old design
# does not count as a writer; and the total is printed so a grep that found
# nothing cannot be mistaken for a subsystem that is clean.
RT_FILES=$(files_matching 'reset_token' $BACKEND_RS)
RT_TOTAL=$(echo "$BACKEND_RS" | wc -l)
if [ "$RT_TOTAL" -lt 10 ]; then
  bad "B2 only ${RT_TOTAL} backend source files found — this arm is measuring nothing"
elif [ "$RT_FILES" = "panel/backend/src/models.rs panel/backend/src/routes/auth.rs " ]; then
  ok "B2 across ${RT_TOTAL} backend files, reset_token is named only by auth.rs and models.rs"
else
  bad "B2 reset_token is named outside the password-reset flow: ${RT_FILES:-<none>}"
fi

echo "§C  the restore never invents a role"

# ⚠ §C USED TO PIN A DECLARATION AND NOT ITS USE, and was green with the
# escalation fully restored. It asserted the value of a `LEAST_PRIVILEGED_ROLE`
# constant; changing the call site to `.unwrap_or_else(|| "user".to_string())`
# left the constant untouched and every arm passing. A pin on a name proves a
# name. These arms are on the decision itself, which is now a pure function
# (`helpers::role_to_restore`) precisely so it can be both pinned and unit-tested
# — the async DB-bound wrapper can be neither.

RTR=$(fn_body '^pub fn role_to_restore' "$HELPERS_S")
UNSUS=$(fn_body '^pub async fn unsuspend_account' "$HELPERS_S")

# C1 — THE ARM. No branch anywhere in the decision may produce a role the caller
# did not record. Any `unwrap_or`/`unwrap_or_else`/`unwrap_or_default` here is a
# default, and a default is a guess.
if [ -z "$RTR" ]; then
  bad "C1 could not extract role_to_restore — this arm is measuring nothing"
elif ! has "$RTR" 'stashed'; then
  bad "C1 the extracted window is not role_to_restore — this arm is measuring nothing"
elif has "$RTR" 'unwrap_or'; then
  bad "C1 the restore decision has a default — an unknown prior role would be GUESSED, which is the defect this suite exists to prevent"
elif has "$RTR" '"(user|admin|reseller|client)"'; then
  bad "C1 the restore decision names a role literal — it must only ever return what was recorded"
else
  ok "C1 the restore decision has no default and names no role literal — an unknown stash cannot be guessed"
fi

# C2 — the decision is actually the one the caller uses. C1 could be pinning a
# pure function nothing calls.
if [ -z "$UNSUS" ]; then
  bad "C2 could not extract unsuspend_account — this arm is measuring nothing"
elif ! has "$UNSUS" 'role_to_restore\('; then
  bad "C2 unsuspend_account does not use role_to_restore — the pinned decision is dead code and the live one is unpinned"
elif has "$UNSUS" 'unwrap_or'; then
  bad "C2 unsuspend_account applies a default of its own, bypassing the pinned decision"
else
  ok "C2 unsuspend_account reaches the pinned decision and adds no default of its own"
fi

# C2b — the stash is validated against the assignable set, so a leftover 64-char
# reset token is never written into a CHECK-constrained column (a 500 on the
# button an administrator presses to undo their own action).
if [ -z "$RTR" ]; then
  : # already reported by C1
elif has "$RTR" 'ASSIGNABLE_ROLES.contains'; then
  ok "C2b a stashed value is validated against ASSIGNABLE_ROLES before being restored"
else
  bad "C2b the restore does not validate the stash — a leftover reset token would be written into a CHECK-constrained column"
fi

# C3 — the stash is consumed. A stash left behind is applied again by a later
# un-suspend, silently overriding a role an administrator chose in between.
if [ -z "$HELPERS_S" ]; then
  : # already reported by C1
elif has "$HELPERS_S" 'prior_role = NULL'; then
  ok "C3 the restore clears the stash in the statement that consumes it"
else
  bad "C3 the restore never clears prior_role — a stale stash will be re-applied by the next un-suspend"
fi

# C4 — suspending an already-suspended account must not overwrite a good stash
# with the status word, which would make the role unrecoverable for ever.
if [ -z "$HELPERS_S" ]; then
  : # already reported by C1
elif has "$HELPERS_S" "role <> 'suspended'"; then
  ok "C4 a re-suspend cannot overwrite a good stash with the status word"
else
  bad "C4 the suspend statement has no role <> 'suspended' guard — re-suspending destroys the stash permanently"
fi

# C5 — the billing webhook may not RESTORE a privileged role. Its suspend arm has
# always refused to touch an `admin`; without a matching deny-list on the way back
# the same webhook secret hands `admin` to an account the PANEL suspended, with no
# activity-log row anywhere. That direction never had a guard at all, and the
# shared helper made it reachable for the first time.
if [ -z "$WHMCS_S" ]; then
  bad "C5 could not read $WHMCS"
elif ! has "$WHMCS_S" 'unsuspend_account'; then
  bad "C5 whmcs does not un-suspend at all — this arm is measuring nothing"
elif has "$WHMCS_S" 'unsuspend_account\(&state, user_id, &\["admin", "reseller"\]\)'; then
  ok "C5 the billing webhook cannot restore a privileged role"
else
  bad "C5 the billing webhook restores whatever was recorded — including admin, to an account the panel suspended"
fi

# C6 — suspending must cut the account's live sessions, and it has to happen HERE
# rather than in each caller. The JWT middleware refuses a token whose CLAIM says
# suspended; a token minted while the account was a `user` keeps saying `user`
# until it expires two hours later. The panel always revoked. The billing webhook
# never did, so a billing suspension did nothing whatsoever for up to two hours —
# on the path the guide describes as following the same rules.
SUSP=$(fn_body '^pub async fn suspend_account' "$HELPERS_S")
if [ -z "$SUSP" ]; then
  bad "C6 could not extract suspend_account — this arm is measuring nothing"
elif has "$SUSP" 'revoke_all_user_sessions'; then
  ok "C6 suspending revokes the account's sessions, for every caller"
else
  bad "C6 suspending does not revoke sessions — a suspended account keeps working until its token expires"
fi

echo "§D  the password-reset doors refuse a suspended account, silently"

# D1/D2 — both doors. `forgot_password` was the ONLY entry point in the whole
# auth surface with no role test: login, the JWT middleware, 2FA, passkeys and
# OAuth all refuse a suspended account explicitly.
if [ -z "$AUTH_S" ]; then
  bad "D1 could not read $AUTH"
else
  N=$(count "$AUTH_S" 'role == "suspended"')
  if [ "$N" -ge 4 ]; then
    ok "D1 the auth surface refuses a suspended account at ${N} distinct doors"
  else
    bad "D1 only ${N} doors in auth.rs test for a suspended role — the reset flow is the one that historically had none"
  fi
fi

# D2 — THE ARMING ARM, and the most important one in this file. The obvious way
# to write D1's guard is an error return. `forgot_password` deliberately answers
# identically for every address to defeat account enumeration; a refusal there
# converts it into an oracle that additionally discloses WHICH addresses are
# suspended, to an unauthenticated caller. That is a worse leak than the bug
# being closed, so the guard must return the success body.
#
# Bound to the forgot_password function rather than the file: a token search
# scoped to a whole file is satisfied by an unrelated occurrence elsewhere, which
# is how B-remove of the transfer pin was green while measuring nothing.
#
FP=$(fn_body '^pub async fn forgot_password' "$AUTH_S")
if [ -z "$FP" ]; then
  bad "D2 could not extract forgot_password — this arm is measuring nothing"
elif ! has "$FP" 'role == "suspended"'; then
  bad "D2 forgot_password does not refuse a suspended account — it is the door with no role check"
elif has "$FP" 'role == "suspended" \{[[:space:]]*$'; then
  # the guard exists; now insist its body returns the success value, not an Err
  if has "$(awk '/role == "suspended"/,/^    \}/' <<< "$FP")" 'return Ok\(Json\(success_msg\)\)'; then
    ok "D2 forgot_password refuses a suspended account SILENTLY — no enumeration oracle"
  else
    bad "D2 forgot_password refuses a suspended account with an error — that discloses which addresses are suspended to an unauthenticated caller"
  fi
else
  bad "D2 forgot_password's suspended guard is not in the expected shape — check it by hand"
fi

# D3 — the second corruptor. A token minted before the account was suspended must
# not be redeemable, and the refusal must read exactly like an unknown token.
RP=$(fn_body '^pub async fn reset_password' "$AUTH_S")
if [ -z "$RP" ]; then
  bad "D3 could not extract reset_password — this arm is measuring nothing"
elif ! has "$RP" 'Invalid or expired reset token'; then
  bad "D3 the extracted window is not reset_password — this arm is measuring nothing"
elif has "$RP" 'role == "suspended"'; then
  ok "D3 reset_password refuses a suspended account, in the same words an unknown token gets"
else
  bad "D3 reset_password does not refuse a suspended account — a token minted before suspension still clears the stash"
fi

echo "§E  one statement each, not a private copy per caller"

# E1 — both suspenders reach the shared statement. This is the arm that would
# catch the next module growing its own copy, which is how the site-ownership
# query became eight helpers under three names, exactly one of which had drifted.
# ⚠ Both arms census CODE, not raw bytes. E1 previously used `grep -rl` and was
# green with every helper call deleted from users.rs, because two doc comments
# there still spelled the helper's name. Each arm also names each helper
# separately: an OR let a file reaching only one of the two count as reaching both.
E1_SUS=$(files_matching 'helpers::suspend_account\(' panel/backend/src/routes/*.rs)
E1_UNS=$(files_matching 'helpers::unsuspend_account\(' panel/backend/src/routes/*.rs)
EXPECTED="panel/backend/src/routes/users.rs panel/backend/src/routes/whmcs.rs "
if [ "$E1_SUS" = "$EXPECTED" ] && [ "$E1_UNS" = "$EXPECTED" ]; then
  ok "E1 the panel and the billing webhook each reach BOTH shared statements"
else
  bad "E1 the suspend paths are not the expected pair — suspend:[${E1_SUS:-<none>}] unsuspend:[${E1_UNS:-<none>}]"
fi

# E2 — nobody writes the suspended status by hand any more.
#
# ⚠ Keyed on the ASSIGNMENT, not on `SET role = 'suspended'`. The first draft
# matched only that exact word order and was green against
# `SET updated_at = NOW(), role = 'suspended'` — the same order-dependence that
# made B1 vacuous. Stripped too, so the comments in whmcs.rs and users.rs that
# discuss the old statement do not count as one.
HANDROLLED=""
for f in $BACKEND_RS; do
  case "$f" in */helpers.rs) continue ;; esac
  if grep -qE "role[[:space:]]*=[[:space:]]*'suspended'" <<< "$(code "$f")"; then HANDROLLED="$HANDROLLED$f "; fi
done
ALLROLE=$(files_matching "SET role" $BACKEND_RS)
if [ -z "$ALLROLE" ]; then
  bad "E2 no 'SET role' statement anywhere in the backend — this arm is measuring nothing"
elif [ -z "$HANDROLLED" ]; then
  ok "E2 only the shared statement writes the suspended status (files writing a role at all: $(echo "$ALLROLE" | wc -w), so the census is live)"
else
  bad "E2 a module still suspends by hand, bypassing the stash: ${HANDROLLED}"
fi

echo
if [ "$FAIL" -eq 0 ]; then
  printf '  \033[32mPASS %d   FAIL %d\033[0m\n' "$PASS" "$FAIL"
else
  printf '  \033[31mPASS %d   FAIL %d\033[0m\n' "$PASS" "$FAIL"
fi
[ "$FAIL" -eq 0 ]
