#!/usr/bin/env bash
# escalation-user-route-scope-pin-e2e.sh — s442
#
# ONE PROPERTY: an escalation-policy `user:<uuid>` step can only ever page a
# user the policy's owner is allowed to page — themselves, or a user they
# directly manage — never an arbitrary third party.
#
# `panel/backend/src/routes/escalation_policies.rs`'s `validate_route` only
# parsed `user:<uuid>` as a UUID shape check. Its sibling shape,
# `on_call_schedule:<uuid>`, got an ownership check at s437
# (`validate_schedule_routes`) after the identical class of bug was found for
# rota references — `user:` never got the matching fix. The gap was live:
# `GET /api/users` lists every user on the install with no scoping (by
# design, for the shared admin directory), so any admin could discover a
# victim's UUID, save an escalation policy with a `user:<victim>` step,
# attach it to an alert rule they own (`attach_escalation_policy` only
# re-checks rule/policy ownership, never the policy's own route steps), and
# trigger it — delivering the alert to the victim's real
# email/Slack/Discord/PagerDuty/webhook with no consent and no way for them
# to trace who set it up.
#
# Fix: `validate_user_routes`, called from both `create_policy` and
# `update_policy` right alongside `validate_schedule_routes`. A `user:`
# target must be the caller themselves OR a user whose `reseller_id` is the
# caller (the same ownership shape `sites.rs`/`databases.rs`/
# `reseller_dashboard.rs` already use for "which users does this admin
# manage").
#
# §A validate_user_routes exists and checks the right table/predicate.
# §B self-routes (paging yourself) need no DB check.
# §C both create_policy and update_policy call it.
# §D position: it runs BEFORE the row is written, alongside
#    validate_schedule_routes (not instead of it).
# §E the "does not exist" non-disclosure convention is preserved (never
#    confirms another user's existence to a non-owner via a different
#    error message).

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  escalation-policy user: route ownership — source pins (s442)"
echo "=================================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

code() {
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has()  { grep -qE -- "$2" <<< "$1"; }

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

offset() {
  local body="$1" needle="$2"
  local head="${body%%"$needle"*}"
  if [ "$head" = "$body" ]; then
    echo -1
  else
    echo "${#head}"
  fi
}

before() {
  local body="$1" first="$2" second="$3"
  local o1 o2
  o1=$(offset "$body" "$first")
  o2=$(offset "$body" "$second")
  [ "$o1" != -1 ] && [ "$o2" != -1 ] && [ "$o1" -lt "$o2" ]
}

EP=panel/backend/src/routes/escalation_policies.rs
EP_C=$(code "$EP")
VALIDATE_USER_BODY=$(fnbody "$EP_C" "validate_user_routes")
CREATE_BODY=$(fnbody "$EP_C" "create_policy")
UPDATE_BODY=$(fnbody "$EP_C" "update_policy")

# ── §A validate_user_routes exists and checks the right ownership predicate ──
echo "── §A validate_user_routes exists, scoped by reseller_id ──"

if [ -n "$VALIDATE_USER_BODY" ]; then
  ok "A1 validate_user_routes exists"
else
  bad "A1 validate_user_routes is missing — every arm below measures nothing"
fi

if has "$VALIDATE_USER_BODY" 'strip_prefix\("user:"\)'; then
  ok "A2 it inspects user: steps specifically"
else
  bad "A2 it doesn't scan for the user: route shape"
fi

if has "$VALIDATE_USER_BODY" 'WHERE id = \$1 AND reseller_id = \$2'; then
  ok "A3 the ownership query matches the codebase's established reseller_id = caller shape"
else
  bad "A3 the ownership predicate is missing or uses a different (unverified) shape"
fi

if has "$VALIDATE_USER_BODY" '\.bind\(target_id\)' && has "$VALIDATE_USER_BODY" '\.bind\(owner_id\)'; then
  ok "A4 the query binds the target and the caller, not some other pair"
else
  bad "A4 the query's bound parameters don't match target_id/owner_id"
fi

# ── §B self-routes need no DB round trip ──────────────────────────────────
echo "── §B paging yourself doesn't need an ownership check ──"

if has "$VALIDATE_USER_BODY" 'target_id == owner_id'; then
  ok "B1 a self-route is special-cased"
else
  bad "B1 no self-route special case — an admin might not even be able to page themselves"
fi

if before "$VALIDATE_USER_BODY" "target_id == owner_id" "sqlx::query_as"; then
  ok "B2 the self-check happens BEFORE the DB round trip (short-circuits it)"
else
  bad "B2 the self-check is positioned after the DB query — dead code or wrong order"
fi

# ── §C both write paths call it ───────────────────────────────────────────
echo "── §C create_policy and update_policy both call validate_user_routes ──"

if has "$CREATE_BODY" 'validate_user_routes\(&state\.db, &input, claims\.sub\)'; then
  ok "C1 create_policy calls validate_user_routes(&state.db, &input, claims.sub)"
else
  bad "C1 create_policy does not call validate_user_routes with the right arguments"
fi

if has "$UPDATE_BODY" 'validate_user_routes\(&state\.db, &input, claims\.sub\)'; then
  ok "C2 update_policy calls validate_user_routes(&state.db, &input, claims.sub)"
else
  bad "C2 update_policy does not call validate_user_routes with the right arguments"
fi

# ── §D position: validated before the write, alongside the schedule check ──
echo "── §D validation runs before the row is written, not instead of the schedule check ──"

if before "$CREATE_BODY" "validate_schedule_routes" "validate_user_routes"; then
  ok "D1 create_policy still calls validate_schedule_routes (not replaced)"
else
  bad "D1 create_policy no longer calls validate_schedule_routes before validate_user_routes — the schedule fix may have been dropped"
fi

if before "$CREATE_BODY" "validate_user_routes" "INSERT INTO escalation_policies"; then
  ok "D2 create_policy validates before the INSERT"
else
  bad "D2 create_policy's validation runs after (or the INSERT doesn't wait for it) — position wrong"
fi

if before "$UPDATE_BODY" "validate_schedule_routes" "validate_user_routes"; then
  ok "D3 update_policy still calls validate_schedule_routes (not replaced)"
else
  bad "D3 update_policy no longer calls validate_schedule_routes before validate_user_routes"
fi

if before "$UPDATE_BODY" "validate_user_routes" "UPDATE escalation_policies"; then
  ok "D4 update_policy validates before the UPDATE"
else
  bad "D4 update_policy's validation runs after (or the UPDATE doesn't wait for it) — position wrong"
fi

# ── §E non-disclosure convention preserved ────────────────────────────────
echo "── §E rejection never confirms another user's existence differently ──"

if has "$VALIDATE_USER_BODY" 'does not exist'; then
  ok "E1 the rejection message matches validate_schedule_routes's non-disclosure phrasing"
else
  bad "E1 rejection message text changed — verify it doesn't leak ownership vs. non-existence"
fi

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
